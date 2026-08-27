use makakoo_core::agents::spec::AgentSpec;

const TEMPLATE: &str = r#"import { createServer } from 'node:http'
import { randomBytes, timingSafeEqual } from 'node:crypto'
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { DeepSeekHarness } from '@deepseek-ai/dsh-sdk-client'

const projectDir = dirname(fileURLToPath(import.meta.url))
// AgentSpec is the source of truth. Project .env files cannot impersonate a
// different slot or silently replace the compiled model/persona.
const slot = __SLOT__
const model = __MODEL__
const systemPrompt = __PROMPT__
const home = process.env.MAKAKOO_HOME
if (!home) throw new Error('MAKAKOO_HOME is required')

const runtimeBin = join(projectDir, 'node_modules/@deepseek-ai/dsh-sdk-jsonrpc-demo/lib/bin.js')
const cordis = join(projectDir, 'cordis.yml')
const sessionRoot = join(projectDir, '.sessions')
const sessionLimitsFile = join(sessionRoot, '.makakoo-limits.json')
const runtimeInfo = join(projectDir, 'runtime.json')
const tokenFile = join(projectDir, '.runtime-token')
if (!existsSync(runtimeBin)) throw new Error('DSH dependencies missing; run npm install in the agent project')
mkdirSync(sessionRoot, { recursive: true, mode: 0o700 })

function boundedInteger(name, fallback, minimum, maximum) {
  const value = Number(process.env[name] ?? fallback)
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new Error(`${name} must be an integer from ${minimum} through ${maximum}`)
  }
  return value
}

function loadToken() {
  const value = randomBytes(32).toString('hex')
  const temporary = `${tokenFile}.tmp`
  rmSync(temporary, { force: true })
  writeFileSync(temporary, value + '\n', { flag: 'wx', mode: 0o600 })
  renameSync(temporary, tokenFile)
  chmodSync(tokenFile, 0o600)
  return value
}

const token = loadToken()
const childEnv = {
  ...process.env,
  MAKAKOO_HOME: home,
  MAKAKOO_AGENT_SLOT: slot,
  MAKAKOO_MCP_BIN: process.env.MAKAKOO_MCP_BIN ?? 'makakoo-mcp',
  // Makakoo's DSH contract is switchAILocal-only. Generated project env cannot redirect it.
  DEEPSEEK_BASE_URL: 'http://127.0.0.1:18080/v1',
  DEEPSEEK_API_KEY: process.env.DEEPSEEK_API_KEY ?? process.env.AIL_API_KEY ?? 'makakoo-local',
  DSH_MODEL: model,
  DSH_CONTEXT_WINDOW: process.env.DSH_CONTEXT_WINDOW ?? '262144',
  DSH_SYSTEM_PROMPT: systemPrompt,
  DSH_SESSION_ROOT: sessionRoot,
}
const maxTokens = boundedInteger('DSH_MAX_TOKENS', '8192', 1, 65536)
const maxConcurrent = boundedInteger('MAKAKOO_DSH_MAX_CONCURRENT', '4', 1, 32)
const maxQueued = boundedInteger('MAKAKOO_DSH_MAX_QUEUED', '32', 0, 1024)
const maxSessions = boundedInteger('MAKAKOO_DSH_MAX_SESSIONS', '128', 1, 4096)
const maxTurnsPerSession = boundedInteger('MAKAKOO_DSH_MAX_TURNS_PER_SESSION', '1000', 1, 100000)
const maxSessionBytes = boundedInteger(
  'MAKAKOO_DSH_MAX_SESSION_BYTES',
  String(512 * 1024 * 1024),
  1024 * 1024,
  16 * 1024 * 1024 * 1024,
)
const maxPromptBytes = boundedInteger(
  'MAKAKOO_DSH_MAX_PROMPT_BYTES',
  String(128 * 1024),
  1024,
  1024 * 1024,
)
const harness = new DeepSeekHarness({
  launch: {
    command: process.execPath,
    args: [runtimeBin, cordis],
    cwd: projectDir,
    env: childEnv,
    shutdownTimeoutMs: 2000,
  },
  cwd: process.env.MAKAKOO_AGENT_CWD ?? home,
  provider: 'deepseek-official',
  model,
  maxTokens,
})

const queues = new Map()
let activeRuns = 0
const runWaiters = []

class RuntimeLimitError extends Error {
  constructor(message, status = 429) {
    super(message)
    this.status = status
  }
}

function directoryBytes(root) {
  if (!existsSync(root)) return 0
  let total = 0
  const pending = [root]
  while (pending.length > 0) {
    const current = pending.pop()
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name)
      if (entry.isSymbolicLink()) throw new Error(`session storage contains forbidden symlink: ${path}`)
      if (entry.isDirectory()) pending.push(path)
      else if (entry.isFile()) total += lstatSync(path).size
    }
  }
  return total
}

function loadSessionLimits() {
  if (!existsSync(sessionLimitsFile)) return new Map()
  const parsed = JSON.parse(readFileSync(sessionLimitsFile, 'utf8'))
  if (parsed?.version !== 1 || typeof parsed.sessions !== 'object' || parsed.sessions === null) {
    throw new Error('invalid durable session limit registry')
  }
  const entries = Object.entries(parsed.sessions)
  if (entries.length > maxSessions) throw new Error('durable session registry exceeds configured maximum')
  const limits = new Map()
  for (const [sessionId, turns] of entries) {
    if (!validSessionId(sessionId) || !Number.isInteger(turns) || turns < 1 || turns > maxTurnsPerSession) {
      throw new Error('invalid entry in durable session limit registry')
    }
    limits.set(sessionId, turns)
  }
  return limits
}

const durableSessions = loadSessionLimits()

function persistSessionLimits() {
  const temporary = `${sessionLimitsFile}.tmp`
  const sessions = Object.fromEntries([...durableSessions.entries()].sort(([a], [b]) => a.localeCompare(b)))
  rmSync(temporary, { force: true })
  writeFileSync(temporary, JSON.stringify({ version: 1, sessions }, null, 2) + '\n', {
    flag: 'wx',
    mode: 0o600,
  })
  renameSync(temporary, sessionLimitsFile)
  chmodSync(sessionLimitsFile, 0o600)
}

function admitSessionTurn(sessionId) {
  if (directoryBytes(sessionRoot) >= maxSessionBytes) {
    throw new RuntimeLimitError('durable session storage limit reached', 507)
  }
  const turns = durableSessions.get(sessionId)
  if (turns === undefined && durableSessions.size >= maxSessions) {
    throw new RuntimeLimitError('durable session count limit reached')
  }
  if ((turns ?? 0) >= maxTurnsPerSession) {
    throw new RuntimeLimitError('durable session turn limit reached')
  }
  durableSessions.set(sessionId, (turns ?? 0) + 1)
  try {
    persistSessionLimits()
  } catch (error) {
    if (turns === undefined) durableSessions.delete(sessionId)
    else durableSessions.set(sessionId, turns)
    throw error
  }
}

function rollbackSessionTurn(sessionId, previousTurns) {
  if (previousTurns === undefined) durableSessions.delete(sessionId)
  else durableSessions.set(sessionId, previousTurns)
  try {
    persistSessionLimits()
    return true
  } catch (rollbackError) {
    console.error(`failed to persist session-turn rollback: ${rollbackError instanceof Error ? rollbackError.message : String(rollbackError)}`)
    return false
  }
}

async function withRunPermit(task) {
  if (activeRuns >= maxConcurrent) {
    if (runWaiters.length >= maxQueued) throw new RuntimeLimitError('runtime concurrency queue full')
    await new Promise(resolve => runWaiters.push(resolve))
  }
  activeRuns += 1
  try {
    return await task()
  } finally {
    activeRuns -= 1
    runWaiters.shift()?.()
  }
}

async function serializeSession(sessionId, task) {
  const previous = queues.get(sessionId) ?? Promise.resolve()
  const current = previous.catch(() => undefined).then(task)
  queues.set(sessionId, current)
  try {
    return await current
  } finally {
    if (queues.get(sessionId) === current) queues.delete(sessionId)
  }
}

function send(response, status, body) {
  const payload = JSON.stringify(body)
  response.writeHead(status, {
    'content-type': 'application/json',
    'content-length': Buffer.byteLength(payload),
  })
  response.end(payload)
}

async function bodyOf(request) {
  const chunks = []
  let size = 0
  for await (const chunk of request) {
    size += chunk.length
    if (size > 1024 * 1024) throw new RuntimeLimitError('request body exceeds 1 MiB', 413)
    chunks.push(chunk)
  }
  return JSON.parse(Buffer.concat(chunks).toString('utf8'))
}

function validSessionId(value) {
  return typeof value === 'string'
    && value !== '.'
    && value !== '..'
    && /^[A-Za-z0-9._:-]{1,128}$/.test(value)
}

function authorized(value) {
  const expected = Buffer.from(`Bearer ${token}`)
  const actual = Buffer.from(value ?? '')
  return actual.length === expected.length && timingSafeEqual(actual, expected)
}

const server = createServer(async (request, response) => {
  if (request.method === 'GET' && request.url === '/health') {
    return send(response, 200, { ok: true, slot, engine: 'deepseek-harness' })
  }
  if (request.method !== 'POST' || request.url !== '/v1/run') {
    return send(response, 404, { error: 'not found' })
  }
  if (!authorized(request.headers.authorization)) {
    return send(response, 401, { error: 'unauthorized' })
  }
  try {
    const body = await bodyOf(request)
    if (typeof body.text !== 'string' || body.text.trim() === '') {
      return send(response, 400, { error: 'text must be a non-empty string' })
    }
    if (Buffer.byteLength(body.text, 'utf8') > maxPromptBytes) {
      return send(response, 413, { error: 'text exceeds configured prompt limit' })
    }
    const sessionId = body.session_id ?? 'api-default'
    if (!validSessionId(sessionId)) {
      return send(response, 400, { error: 'invalid session_id' })
    }
    const result = await serializeSession(
      sessionId,
      () => withRunPermit(async () => {
        const previousTurns = durableSessions.get(sessionId)
        admitSessionTurn(sessionId)
        try {
          return await harness.run(body.text, { sessionId })
        } catch (error) {
          rollbackSessionTurn(sessionId, previousTurns)
          // Rollback persistence errors are reported by the helper but must
          // never replace the original harness failure returned to the caller.
          throw error
        }
      }),
    )
    send(response, 200, { session_id: result.sessionId, response: result.finalResponse })
  } catch (error) {
    const status = error instanceof RuntimeLimitError ? error.status : 500
    send(response, status, { error: error instanceof Error ? error.message : String(error) })
  }
})

let stopping = false
async function shutdown(code) {
  if (stopping) return
  stopping = true
  clearInterval(parentWatch)
  const forceClose = setTimeout(() => server.closeAllConnections(), 750)
  forceClose.unref()
  await new Promise(resolve => server.close(resolve))
  clearTimeout(forceClose)
  await harness.close()
  rmSync(runtimeInfo, { force: true })
  rmSync(tokenFile, { force: true })
  process.exit(code)
}
process.on('SIGTERM', () => { void shutdown(0) })
process.on('SIGINT', () => { void shutdown(130) })
const supervisorPid = process.ppid
const parentWatch = setInterval(() => {
  try {
    if (process.ppid !== supervisorPid) throw new Error('runtime reparented')
    process.kill(supervisorPid, 0)
  } catch {
    void shutdown(1)
  }
}, 1000)
parentWatch.unref()

await harness.start()
const requestedPort = Number(process.env.MAKAKOO_DSH_PORT ?? '0')
if (!Number.isInteger(requestedPort) || requestedPort < 0 || requestedPort > 65535) {
  throw new Error('MAKAKOO_DSH_PORT must be an integer from 0 through 65535')
}
await new Promise((resolve, reject) => {
  server.once('error', reject)
  server.listen(requestedPort, '127.0.0.1', resolve)
})
const address = server.address()
if (!address || typeof address === 'string') throw new Error('runtime did not bind TCP')
const info = { slot, engine: 'deepseek-harness', host: '127.0.0.1', port: address.port, pid: process.pid, token_file: tokenFile }
rmSync(`${runtimeInfo}.tmp`, { force: true })
writeFileSync(`${runtimeInfo}.tmp`, JSON.stringify(info, null, 2) + '\n', { mode: 0o600 })
renameSync(`${runtimeInfo}.tmp`, runtimeInfo)
chmodSync(runtimeInfo, 0o600)
process.stderr.write(`makakoo dsh runtime ${slot} listening on 127.0.0.1:${address.port}\n`)
"#;

pub fn render(spec: &AgentSpec) -> String {
    let prompt = format!(
        "You are {}. Your Makakoo slot id is {}.\n\n{}",
        spec.name, spec.name, spec.instructions
    );
    TEMPLATE
        .replace("__SLOT__", &json_string(&spec.name))
        .replace("__MODEL__", &json_string(model_id(&spec.model)))
        .replace("__PROMPT__", &json_string(&prompt))
}

fn model_id(model: &str) -> &str {
    model.split_once('/').map(|(_, id)| id).unwrap_or(model)
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("string JSON serialization")
}

#[cfg(test)]
mod tests {
    use super::*;
    use makakoo_core::agents::spec::ScopeSpec;

    #[test]
    fn model_provider_prefix_is_removed_for_switchailocal_route() {
        let spec = AgentSpec {
            name: "researcher".into(),
            description: "Research".into(),
            model: "switchailocal/ail-compound".into(),
            instructions: "Use evidence.".into(),
            tools: vec![],
            channels: vec![],
            triggers: vec![],
            scope: ScopeSpec::default(),
        };
        let output = render(&spec);
        assert!(output.contains("const model = \"ail-compound\""));
        assert!(output.contains("const slot = \"researcher\""));
        assert!(output.contains("Your Makakoo slot id is researcher"));
        assert!(!output.contains("switchailocal/ail-compound"));
        assert!(output.contains("MAKAKOO_DSH_MAX_SESSION_BYTES"));
        assert!(output.contains("MAKAKOO_DSH_MAX_TURNS_PER_SESSION"));
        assert!(output.contains("admitSessionTurn(sessionId)"));
        assert!(output.contains("rollbackSessionTurn(sessionId, previousTurns)"));
        assert!(output.contains("failed to persist session-turn rollback"));
        assert!(output.contains("chmodSync(runtimeInfo, 0o600)"));
        assert!(output.contains("process.ppid !== supervisorPid"));
    }
}
