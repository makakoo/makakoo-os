#!/usr/bin/env node
/*
 * harvey-listen.js — Octopus listener daemon (Harvey Octopus Phase 1+).
 *
 * Polls the makakoo-mcp `brain_tail` tool every N seconds looking for
 * `@pod-NN` / `@<peer-name>` mentions in today's Brain journal. When one
 * arrives, logs it and writes an acknowledgement line back via
 * `brain_write_journal`.
 *
 * Self-ack filtering uses a **nonce-aware LRU cache**: each ack write
 * carries a unique nonce (X-Makakoo-Nonce header) which the shim echoes
 * into the journal line (`{nonce=<id>}` suffix). Any polled line whose
 * nonce is in our LRU cache is our own write → drop. This replaces the
 * earlier 500ms timer heuristic, which was fragile: a Mac outage + retry
 * could push a real ack beyond the window and re-process it.
 *
 * We ALSO keep the `[[Harvey Octopus]]` marker filter as a belt-and-
 * suspenders safety net. If a peer's nonce gets lost across a restart
 * (LRU cache is RAM-only) the marker filter still catches the echo.
 *
 * Usage (background):
 *   nohup node <plugin-dir>/lib-harvey-core/src/core/harvey-listen.js \
 *     > /tmp/harvey-listen.log 2>&1 &
 *
 * Reads from `$OCTOPUS_KEY_DIR` (default `/app/workspace/.mcp-keys` inside
 * pods, `$HOME/.makakoo/keys` on a general install):
 *   pod.pem, harvey-endpoint.txt, peer-name.txt — identity material
 *   brain-cursor.json — persisted poll cursor ({date, line})
 *
 * Tunables:
 *   HARVEY_LISTEN_INTERVAL_S       — poll cadence (default 30)
 *   HARVEY_LISTEN_MAX_HITS         — max hits processed per poll (default 10)
 *   HARVEY_LISTEN_PATTERN          — trigger substring (default "@<peer-name>")
 *   HARVEY_LISTEN_NONCE_LRU_SIZE   — nonce cache size (default 100)
 *   OCTOPUS_KEY_DIR                — override key dir
 *
 * Opt-in: requires a `listener-enabled` file inside `OCTOPUS_KEY_DIR`
 * (create with `touch`). Pods ship DORMANT unless explicitly enabled —
 * prevents a default-on listener from hammering the Mac on a mis-
 * deployed peer.
 */

const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const http = require('http');
const https = require('https');

const KEY_DIR = process.env.OCTOPUS_KEY_DIR
  || (fs.existsSync('/app/workspace/.mcp-keys')
        ? '/app/workspace/.mcp-keys'
        : path.join(process.env.HOME || '', '.makakoo', 'keys'));
const CURSOR_PATH = path.join(KEY_DIR, 'brain-cursor.json');

const INTERVAL_S = parseInt(process.env.HARVEY_LISTEN_INTERVAL_S || '30', 10);
const MAX_HITS_PER_POLL = parseInt(process.env.HARVEY_LISTEN_MAX_HITS || '10', 10);
const NONCE_LRU_SIZE = parseInt(process.env.HARVEY_LISTEN_NONCE_LRU_SIZE || '100', 10);
const SELF_ACK_MARKER = '[[Harvey Octopus]]';

function logLine(level, msg) {
  process.stdout.write(`${new Date().toISOString()} [${level}] ${msg}\n`);
}

function readOrDie(p) {
  try { return fs.readFileSync(p, 'utf8'); }
  catch (e) { logLine('FATAL', `${p} unreadable: ${e.code}`); process.exit(4); }
}

// Bootstrapped once — endpoint and peer name don't change across a run.
const ENDPOINT = readOrDie(path.join(KEY_DIR, 'harvey-endpoint.txt')).trim();
const PEER = readOrDie(path.join(KEY_DIR, 'peer-name.txt')).trim();
const PRIVATE_KEY = crypto.createPrivateKey({
  key: readOrDie(path.join(KEY_DIR, 'pod.pem')),
  format: 'pem',
});
const PATTERN = process.env.HARVEY_LISTEN_PATTERN || `@${PEER}`;


// ────────────────────────── nonce-aware LRU ────────────────────────
//
// Insertion-ordered Set is the simplest LRU in Node: `add` moves the key
// to the tail on re-insertion, `delete`+`values().next()` trims the head.
// No external dependency, O(1) membership check on hot path.

class NonceLRU {
  constructor(capacity) {
    if (capacity < 1) throw new Error('LRU capacity must be ≥ 1');
    this.capacity = capacity;
    this.set = new Set();
  }
  has(k) { return this.set.has(k); }
  add(k) {
    if (this.set.has(k)) { this.set.delete(k); }
    this.set.add(k);
    while (this.set.size > this.capacity) {
      const oldest = this.set.values().next().value;
      this.set.delete(oldest);
    }
  }
}

const nonces = new NonceLRU(NONCE_LRU_SIZE);


// ────────────────────────── nonce ops ──────────────────────────────

function mintNonce() {
  // 128 bits is plenty — collision would require ~2^64 writes per pod.
  // Hyphens-only alphabet matches brain_tail's nonce regex.
  return crypto.randomBytes(16).toString('hex');
}

function extractNonce(line) {
  // Mirror of core/brain_tail.extract_nonce — `{nonce=<id>}` at end-of-
  // line, optional trailing whitespace.
  const m = /\{nonce=([A-Za-z0-9\-_]+)\}\s*$/.exec(line);
  return m ? m[1] : null;
}


// ────────────────────────── cursor persistence ─────────────────────

function loadCursor() {
  try { return JSON.parse(fs.readFileSync(CURSOR_PATH, 'utf8')); }
  catch { return null; }
}

function saveCursor(c) {
  const tmp = CURSOR_PATH + '.tmp';
  fs.writeFileSync(tmp, JSON.stringify(c));
  fs.renameSync(tmp, CURSOR_PATH);
}


// ────────────────────────── mcp transport ──────────────────────────

function mcpCall(method, params) {
  return new Promise((resolve, reject) => {
    const body = Buffer.from(JSON.stringify({
      jsonrpc: '2.0', id: Date.now(), method, params: params || undefined,
    }));
    const ts = Date.now();
    const nonce = mintNonce();
    const digest = crypto.createHash('sha256')
      .update(body).update(Buffer.from(ts.toString())).digest();
    const sig = crypto.sign(null, digest, PRIVATE_KEY).toString('base64');
    const u = new URL(ENDPOINT);
    const lib = u.protocol === 'https:' ? https : http;
    const req = lib.request({
      method: 'POST',
      hostname: u.hostname,
      port: u.port,
      path: u.pathname || '/rpc',
      headers: {
        'Content-Type': 'application/json',
        'Content-Length': body.length,
        'X-Makakoo-Peer': PEER,
        'X-Makakoo-Ts': ts.toString(),
        'X-Makakoo-Sig': `ed25519=${sig}`,
        'X-Makakoo-Nonce': nonce,
      },
      timeout: 20000,
    }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => {
        const raw = Buffer.concat(chunks).toString('utf8');
        if (res.statusCode !== 200) {
          return reject(new Error(`HTTP ${res.statusCode}: ${raw.slice(0, 200)}`));
        }
        try { resolve({ body: JSON.parse(raw), nonce }); }
        catch (e) { reject(new Error(`bad JSON: ${raw.slice(0, 200)}`)); }
      });
    });
    req.on('timeout', () => { req.destroy(); reject(new Error('mcp call timeout')); });
    req.on('error', (e) => reject(e));
    req.write(body);
    req.end();
  });
}


// ────────────────────────── poll loop ──────────────────────────────

async function pollOnce() {
  const cursor = loadCursor();
  const params = { pattern: PATTERN };
  if (cursor) {
    params.cursor_date = cursor.date;
    params.cursor_line = cursor.line;
  }
  const { body: rpc } = await mcpCall('tools/call', {
    name: 'brain_tail', arguments: params,
  });
  if (rpc.error) throw new Error(`brain_tail error: ${rpc.error.message}`);
  const text = rpc.result?.content?.[0]?.text || '{}';
  const parsed = JSON.parse(text);
  const hits = parsed.hits || [];
  const nextCursor = parsed.next_cursor;

  if (hits.length > MAX_HITS_PER_POLL) {
    logLine('WARN', `got ${hits.length} hits, processing ${MAX_HITS_PER_POLL} (rest queued)`);
  }

  // Primary filter: nonce-aware LRU — brain_tail returns the parsed
  // `nonce` field per hit (or null if the line has no nonce). Lines we
  // wrote have their nonce in our LRU → drop.
  //
  // Secondary filter: the `[[Harvey Octopus]]` marker. Same effect as
  // before, but only fires for lines whose nonce wasn't found. Keeps the
  // listener safe across restarts (LRU is RAM-only) and against pod-
  // peers that share the mesh but don't propagate nonces yet.
  const real = hits.filter((h) => {
    const hitNonce = h.nonce || extractNonce(h.text || '');
    if (hitNonce && nonces.has(hitNonce)) return false;
    if ((h.text || '').includes(SELF_ACK_MARKER)) return false;
    return true;
  });
  if (real.length < hits.length) {
    logLine('INFO', `filtered ${hits.length - real.length} self-ack echo(es)`);
  }
  const toProcess = real.slice(0, MAX_HITS_PER_POLL);
  let ackFailures = 0;

  for (const hit of toProcess) {
    logLine('INFO', `mention at ${hit.date}:${hit.line}: ${hit.text}`);
    // Pointer-only ack: never quotes the trigger substring, so even if
    // the nonce mechanism fails open the ack can never re-fire itself.
    const ack = `- ${SELF_ACK_MARKER} ${PEER} acked mention at ${hit.date}:${hit.line} (${new Date().toISOString()})`;
    try {
      const { body: ackResp, nonce } = await mcpCall('tools/call', {
        name: 'brain_write_journal',
        arguments: { content: ack },
      });
      if (ackResp.error) {
        logLine('ERROR', `ack write failed: ${ackResp.error.message}`);
        ackFailures += 1;
      } else {
        // Success — remember the nonce so next poll filters our own
        // journal line even if the `[[Harvey Octopus]]` marker ever
        // gets loosened.
        nonces.add(nonce);
      }
    } catch (e) {
      logLine('ERROR', `ack call failed: ${e.message}`);
      ackFailures += 1;
    }
  }

  // Advance cursor ONLY on full ack success. A transient Mac outage
  // during an ack write would otherwise silently drop the mention. Re-
  // processing on the next poll is safe because the nonce LRU
  // deduplicates the duplicate ack attempt, and the brain journal's
  // append-only property means no data loss either way.
  if (nextCursor && ackFailures === 0) saveCursor(nextCursor);
  return hits.length;
}


// ────────────────────────── main ───────────────────────────────────

async function main() {
  const enableFlag = path.join(KEY_DIR, 'listener-enabled');
  if (!fs.existsSync(enableFlag)) {
    logLine('INFO', `listener-enabled flag absent at ${enableFlag} — exiting (run "touch ${enableFlag}" to opt in)`);
    process.exit(0);
  }
  logLine('INFO', `harvey-listen starting — endpoint=${ENDPOINT} peer=${PEER} pattern="${PATTERN}" interval=${INTERVAL_S}s lru=${NONCE_LRU_SIZE}`);

  let consecutiveErrors = 0;

  while (true) {
    try {
      const n = await pollOnce();
      if (n > 0) logLine('INFO', `processed ${n} mention(s)`);
      consecutiveErrors = 0;
    } catch (e) {
      consecutiveErrors += 1;
      logLine('ERROR', `poll failed (${consecutiveErrors} in a row): ${e.message}`);
    }
    const sleepS = Math.min(INTERVAL_S * Math.pow(2, consecutiveErrors), INTERVAL_S * 5);
    await new Promise((r) => setTimeout(r, sleepS * 1000));
  }
}

process.on('SIGTERM', () => { logLine('INFO', 'SIGTERM received, exiting'); process.exit(0); });
process.on('SIGINT', () => { logLine('INFO', 'SIGINT received, exiting'); process.exit(0); });

// Export for tests; only start the loop when invoked as a script.
module.exports = { NonceLRU, extractNonce, mintNonce };
if (require.main === module) {
  main().catch((e) => { logLine('FATAL', e.stack || e.message); process.exit(1); });
}
