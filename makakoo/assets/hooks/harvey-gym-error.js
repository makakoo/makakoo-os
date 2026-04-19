#!/usr/bin/env node
// harvey-gym-error.js — Claude Code Stop hook for the Harvey Mascot GYM
//
// Fires at the end of each assistant turn. Inspects the transcript file that
// Claude Code passes in and extracts tool failures (tool results with error
// markers or non-zero exits), then appends one JSONL record per failure to
//   $HARVEY_HOME/data/errors/YYYY-MM-DD/tools.jsonl
//
// Transparent: never blocks the response, never raises to the user. Any
// internal failure is swallowed with a silent stderr warning.
//
// Input (from Claude Code):
//   stdin JSON: {
//     "session_id": "...",
//     "transcript_path": "/path/to/session.jsonl",
//     "stop_hook_active": true|false
//   }

const fs = require('fs');
const path = require('path');
const os = require('os');

const HARVEY_HOME = process.env.MAKAKOO_HOME
    || process.env.HARVEY_HOME
    || path.join(os.homedir(), 'MAKAKOO');
const HOME = os.homedir();

const BENIGN_STDERR_RE = [
    /shell cwd was reset/i,
    /operation was aborted/i,
    /interrupted by user/i,
    /^\s*$/,
];

function isBenignStderr(s) {
    if (!s || typeof s !== 'string') return true;
    const trimmed = s.trim();
    if (!trimmed) return true;
    return BENIGN_STDERR_RE.some((re) => re.test(trimmed));
}

function isoNow() {
    return new Date().toISOString();
}

function todayUTC() {
    return new Date().toISOString().slice(0, 10);
}

function redact(s) {
    if (!s) return '';
    return String(s).split(HOME).join('$HOME');
}

function truncate(s, limit) {
    if (!s) return '';
    if (s.length <= limit) return s;
    return s.slice(0, limit - 15) + '...[truncated]';
}

function logFile() {
    const dir = path.join(HARVEY_HOME, 'data', 'errors', todayUTC());
    try {
        fs.mkdirSync(dir, { recursive: true });
    } catch (_) {}
    return path.join(dir, 'tools.jsonl');
}

function writeRecord(record) {
    try {
        fs.appendFileSync(logFile(), JSON.stringify(record) + '\n', 'utf8');
    } catch (err) {
        process.stderr.write(`harvey-gym-error: write failed: ${err}\n`);
    }
}

function readInput() {
    try {
        const raw = fs.readFileSync(0, 'utf8');
        return raw.trim() ? JSON.parse(raw) : {};
    } catch (_) {
        return {};
    }
}

function extractToolFailures(transcriptPath) {
    if (!transcriptPath || !fs.existsSync(transcriptPath)) return [];
    let lines = [];
    try {
        lines = fs.readFileSync(transcriptPath, 'utf8').split('\n');
    } catch (_) {
        return [];
    }

    // Pass 1: walk forward, build id→name map from assistant tool_use blocks.
    const idToName = {};
    for (const line of lines) {
        if (!line) continue;
        let evt;
        try { evt = JSON.parse(line); } catch (_) { continue; }
        const content = evt?.message?.content;
        if (!Array.isArray(content)) continue;
        for (const block of content) {
            if (block && block.type === 'tool_use' && block.id && block.name) {
                idToName[block.id] = block.name;
            }
        }
    }

    // Pass 2: walk from the end, collect failures from the most-recent turn only.
    // Stop at the first user message (not a tool_result user message — a real one).
    const failures = [];
    for (let i = lines.length - 1; i >= 0; i--) {
        const line = lines[i];
        if (!line) continue;
        let evt;
        try { evt = JSON.parse(line); } catch (_) { continue; }

        // Boundary: a user message that is not wrapping a tool_result
        if (evt.type === 'user' && evt.message && !evt.toolUseResult) {
            const content = evt?.message?.content;
            const looksLikeToolResult = Array.isArray(content)
                && content.some((b) => b && b.type === 'tool_result');
            if (!looksLikeToolResult) break;
        }

        const result = evt.toolUseResult;
        if (!result) continue;

        const rawStderr = typeof result.stderr === 'string' ? result.stderr : '';
        const stderrMeaningful = rawStderr.length > 0 && !isBenignStderr(rawStderr);
        // Treat code as a shell exit code only if it looks like one (1-127).
        // HTTP status codes (100-599) are NOT errors — they leak from MCP/API results.
        const code = typeof result.code === 'number' ? result.code : null;
        const isShellError = code !== null && code !== 0 && code < 128;
        const isError = result.is_error === true
            || result.isError === true
            || stderrMeaningful
            || isShellError;

        if (!isError) continue;
        if (!stderrMeaningful && result.interrupted === true) continue;

        // Resolve tool name: try every field Claude Code might use.
        const toolUseId = evt.toolUseID || evt.tool_use_id
            || result.tool_use_id || null;
        const toolName = (toolUseId && idToName[toolUseId])
            || evt.toolName || evt.name
            || result.tool || result.name
            || 'unknown_tool';

        const stderr = result.stderr || result.error || result.content?.[0]?.text || '';

        failures.push({
            tool: toolName,
            stderr: typeof stderr === 'string' ? stderr : JSON.stringify(stderr).slice(0, 2048),
            code: typeof result.code === 'number' ? result.code : null,
            tool_use_id: toolUseId,
        });
    }
    return failures.reverse();
}

function main() {
    const input = readInput();
    const transcript = input.transcript_path;
    const failures = extractToolFailures(transcript);

    for (const f of failures) {
        const record = {
            schema_version: '1.0',
            ts: isoNow(),
            source: 'tool',
            cmd: truncate(redact(f.tool), 512),
            cwd: redact(process.cwd()),
            stderr: truncate(redact(f.stderr), 2048),
            exit_code: f.code,
            agent: process.env.HARVEY_AGENT || 'harvey',
            skill_in_scope: process.env.HARVEY_SKILL_IN_SCOPE || null,
            error_class: null,
            raw: {
                tool_use_id: f.tool_use_id,
                session_id: input.session_id || null,
            },
        };
        writeRecord(record);
    }
    // Stop hooks must exit 0 or they will be treated as blocking.
    process.exit(0);
}

try {
    main();
} catch (err) {
    process.stderr.write(`harvey-gym-error: fatal: ${err}\n`);
    process.exit(0);
}
