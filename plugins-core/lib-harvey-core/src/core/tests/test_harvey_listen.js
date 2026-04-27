#!/usr/bin/env node
/*
 * test_harvey_listen.js — fast unit tests for the nonce-aware LRU and the
 * extractNonce/mintNonce helpers. No network, no subprocess.
 *
 * To run the listener itself requires env fixtures (endpoint.txt,
 * peer-name.txt, pod.pem) — that's covered by the integration test
 * under plugins-core/lib-harvey-core/src/core/mcp/tests/ which runs
 * against a live shim. This file exercises only the pure-JS pieces.
 *
 * Run:
 *   node plugins-core/lib-harvey-core/src/core/tests/test_harvey_listen.js
 *
 * Exits non-zero on any assertion failure.
 */

const assert = require('assert');
const path = require('path');

// The listener bails at `readOrDie` if OCTOPUS_KEY_DIR isn't a full
// identity bundle. Point it at this test dir and stub the missing files
// before requiring, so the module's top-level import doesn't fatal.
const os = require('os');
const fs = require('fs');
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'octopus-listen-test-'));
fs.writeFileSync(path.join(tmp, 'harvey-endpoint.txt'), 'http://127.0.0.1:8765/rpc\n');
fs.writeFileSync(path.join(tmp, 'peer-name.txt'), 'test-peer\n');
// Generate a throw-away Ed25519 private key in PEM form just to satisfy
// `crypto.createPrivateKey` at import time.
const crypto = require('crypto');
const { privateKey } = crypto.generateKeyPairSync('ed25519');
fs.writeFileSync(
  path.join(tmp, 'pod.pem'),
  privateKey.export({ type: 'pkcs8', format: 'pem' }),
);
process.env.OCTOPUS_KEY_DIR = tmp;

const listener = require(path.join(__dirname, '..', 'harvey-listen.js'));
const { NonceLRU, extractNonce, mintNonce } = listener;

function test(name, fn) {
  try {
    fn();
    console.log(`PASS  ${name}`);
  } catch (e) {
    console.error(`FAIL  ${name}`);
    console.error(e);
    process.exitCode = 1;
  }
}

// ───── NonceLRU ────────────────────────────────────────────────────

test('LRU evicts oldest when capacity exceeded', () => {
  const lru = new NonceLRU(3);
  lru.add('a'); lru.add('b'); lru.add('c');
  assert.strictEqual(lru.has('a'), true);
  lru.add('d');  // evicts 'a'
  assert.strictEqual(lru.has('a'), false);
  assert.strictEqual(lru.has('b'), true);
  assert.strictEqual(lru.has('c'), true);
  assert.strictEqual(lru.has('d'), true);
});

test('LRU promotes re-inserted key to tail (keeps it alive)', () => {
  const lru = new NonceLRU(3);
  lru.add('a'); lru.add('b'); lru.add('c');
  lru.add('a');          // 'a' is now the newest, 'b' the oldest
  lru.add('d');          // should evict 'b'
  assert.strictEqual(lru.has('b'), false);
  assert.strictEqual(lru.has('a'), true);
  assert.strictEqual(lru.has('c'), true);
  assert.strictEqual(lru.has('d'), true);
});

test('LRU capacity of 1 behaves as single-slot', () => {
  const lru = new NonceLRU(1);
  lru.add('x');
  assert.strictEqual(lru.has('x'), true);
  lru.add('y');
  assert.strictEqual(lru.has('x'), false);
  assert.strictEqual(lru.has('y'), true);
});

test('LRU rejects capacity < 1', () => {
  assert.throws(() => new NonceLRU(0));
  assert.throws(() => new NonceLRU(-1));
});

// ───── extractNonce / mintNonce ─────────────────────────────────────

test('extractNonce matches trailing {nonce=<id>}', () => {
  assert.strictEqual(extractNonce('- hello world {nonce=abc-123}'), 'abc-123');
  assert.strictEqual(extractNonce('- trailing whitespace {nonce=xyz}  '), 'xyz');
});

test('extractNonce returns null when no token present', () => {
  assert.strictEqual(extractNonce('- plain human line'), null);
  assert.strictEqual(extractNonce('- contains {nonce=} empty id'), null);
});

test('extractNonce is symmetric with a minted nonce', () => {
  const n = mintNonce();
  const line = `- some content {nonce=${n}}`;
  assert.strictEqual(extractNonce(line), n);
});

test('mintNonce produces collision-resistant hex', () => {
  const seen = new Set();
  for (let i = 0; i < 2000; i++) {
    const n = mintNonce();
    assert.ok(/^[0-9a-f]{32}$/.test(n), `bad shape: ${n}`);
    assert.ok(!seen.has(n), `dup within 2000 mints: ${n}`);
    seen.add(n);
  }
});

if (process.exitCode === 0 || process.exitCode === undefined) {
  console.log('ALL PASS');
}
