#!/usr/bin/env node
// SPDX-License-Identifier: MPL-2.0

// Real browser-target WebAssembly versus the existing native Rune worker.
// Usage: node scripts/tests/test-preview-wasm.mjs --bin target/<host>/debug/panelet-studio
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawnSync} from 'node:child_process';
import {performance} from 'node:perf_hooks';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const args = process.argv.slice(2);
function option(name, fallback) {
  const index = args.indexOf(name);
  return index < 0 ? fallback : args[index + 1];
}
const binary = option('--bin');
assert(binary, 'Pass --bin pointing to a freshly built native panelet-studio');
const wasmFile = option('--wasm', path.join(root, 'target/web-assets/raw/preview.wasm'));
const module = await WebAssembly.compile(fs.readFileSync(wasmFile));
assert.deepEqual(WebAssembly.Module.imports(module), [{module: 'env', name: 'now_millis', kind: 'function'}]);
const instance = await WebAssembly.instantiate(module, {env: {now_millis: () => performance.now()}});
const {alloc, call, release, memory} = instance.exports;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

function response(descriptor) {
  const ptr = Number(descriptor & 0xffff_ffffn);
  const len = Number(descriptor >> 32n);
  assert(ptr > 0 && len > 0, 'Output allocation must succeed');
  try { return JSON.parse(decoder.decode(new Uint8Array(memory.buffer, ptr, len))); }
  finally { release(ptr, len); }
}
function invoke(op, input) {
  const bytes = input instanceof Uint8Array ? input : encoder.encode(JSON.stringify(input));
  const ptr = alloc(bytes.length);
  assert(ptr > 0, 'Input allocation must succeed');
  try {
    new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
    return response(call(op, ptr, bytes.length));
  } finally { release(ptr, bytes.length); }
}
function native(request) {
  const result = spawnSync(path.resolve(binary), ['--render-worker'], {
    input: JSON.stringify(request), encoding: 'utf8', timeout: 10000, maxBuffer: 1024 * 1024,
  });
  assert.equal(result.status, 0, result.error?.message || result.stderr);
  return JSON.parse(result.stdout);
}
function example(name) {
  const dir = path.join(root, 'examples/widgets', name);
  const resources = [];
  function walk(subdir) {
    for (const entry of fs.readdirSync(subdir, {withFileTypes: true}).sort((a,b) => a.name.localeCompare(b.name))) {
      const filename = path.join(subdir, entry.name);
      if (entry.isDirectory()) walk(filename);
      else resources.push({path: path.relative(path.join(dir, 'resources'), filename).split(path.sep).join('/'), text: fs.readFileSync(filename, 'utf8')});
    }
  }
  if (fs.existsSync(path.join(dir, 'resources'))) walk(path.join(dir, 'resources'));
  const request = {
    package: {manifest: JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8')), source: fs.readFileSync(path.join(dir, 'main.rn'), 'utf8'), resources},
    config: {}, host_permissions: null,
    environment: {unix_ms: Date.parse('2026-09-23T00:00:00Z'), utc_offset_minutes: 480, http: [], shared_files: {}}, events: [],
  };
  if (name === 'todo') {
    const url = 'https://preview.example.test/todo.json';
    request.config = {url, title: '待办事项', auth_type: 'none'};
    request.host_permissions = [url];
    request.environment.http = [{url, status: 200, body: fs.readFileSync(path.join(dir, 'feed.example.json'), 'utf8')}];
  }
  return request;
}
let checks = 0;
function compare(label, request, expectedOk = true) {
  const wasm = invoke(0, request);
  const host = native(request);
  assert.equal(wasm.ok, expectedOk, `${label}: ${JSON.stringify(wasm.error)}`);
  assert.equal(wasm.ok, host.ok, label);
  assert.deepEqual(wasm.pixels, host.pixels, `${label}: exact packed framebuffer differs`);
  assert.deepEqual(wasm.logs, host.logs, `${label}: logs differ`);
  assert.equal(wasm.error?.code, host.error?.code, `${label}: error codes differ`);
  if (expectedOk) assert.equal(wasm.pixels.length, 30_000);
  checks++;
  return wasm;
}
for (const name of ['counter', 'calendar', 'todo', 'photo-album']) {
  const request = example(name);
  compare(name, request);
  const encoded = invoke(1, request.package);
  assert.equal(encoded.ok, true, `${name} encode`);
  const decoded = invoke(2, new Uint8Array(encoded.bytes));
  assert.equal(decoded.ok, true, `${name} decode`);
  assert.deepEqual(decoded.package, request.package, `${name} package round trip`);
  compare(`${name} decoded`, {...request, package: decoded.package});
  const damaged = new Uint8Array(encoded.bytes);
  damaged[50] ^= 1;
  assert.equal(invoke(2, damaged).ok, false, 'CRC/header damage must be rejected');
}
const counter = example('counter');
const baseline = invoke(0, counter);
const pressed = compare('counter input', {...counter, events: [{kind: 'click', key: 'boot'}]});
assert.notDeepEqual(pressed.pixels, baseline.pixels);
assert.deepEqual(invoke(0, counter).pixels, baseline.pixels, 'Each request resets state');
compare('counter long presses', {...counter, events: [{kind: 'long_press', key: 'up'}, {kind: 'long_press', key: 'down'}]});
compare('calendar timer', {...example('calendar'), events: [{kind: 'timer', advance_ms: 86_400_000}]});
compare('compile error', {...counter, package: {...counter.package, source: 'struct State {'}}, false);
compare('capability denied', {...counter, package: {...counter.package, manifest: {...counter.package.manifest, capabilities: []}}}, false);
compare('source limit', {...counter, package: {...counter.package, source: ' '.repeat(5121)}}, false);
compare('event limit', {...counter, events: Array.from({length: 65}, () => ({kind: 'timer'}))}, false);
compare('config validation', {...counter, config: []}, false);
for (const body of ['loop {}', 'let values = []; loop {values.push([0,1,2,3,4,5,6,7,8,9]);}']) {
  const source = `struct State {} #[widget::component] impl State { pub fn new() { State {} } #[widget::mounted] pub async fn m(self) { ${body} os::runtime::ok(()) } }`;
  compare('instruction/allocation limit', {...counter, package: {...counter.package, source}}, false);
}
assert.equal(invoke(0, encoder.encode('{')).error.code, 'invalid_json');
assert.equal(invoke(7, {}).error.code, 'invalid_operation');
assert.equal(alloc(0), 0);
assert.equal(alloc(65537), 0);
assert.equal(response(call(0, 0, 65537)).error.code, 'request_too_large');
assert.equal(response(call(0, 123, 1)).error.code, 'invalid_buffer');
const maxPtr = alloc(65536);
assert(maxPtr > 0);
release(maxPtr, 65535); // Wrong length does not free someone else's buffer.
assert.equal(response(call(0, maxPtr, 65535)).error.code, 'invalid_buffer');
release(maxPtr, 65536);
release(maxPtr, 65536); // Double release is harmless.
const held = Array.from({length: 8}, () => alloc(1));
assert(held.every(ptr => ptr > 0), 'All prior allocations were released');
assert.equal(alloc(1), 0, 'Outstanding allocation count is bounded');
for (const ptr of held) release(ptr, 1);
compare('recovery after rejected requests', counter);
// The linker maximum is part of the deployment resource bound, not a UI promise.
const pages = memory.buffer.byteLength / 65536;
memory.grow(1024 - pages);
assert.throws(() => memory.grow(1), RangeError, 'WASM maximum must be 64 MiB');
console.log(`PASS: ${checks} native/WASM comparisons; four examples, events, package round trips, errors, limits, allocation ownership, 64 MiB memory ceiling.`);
