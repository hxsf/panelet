#!/usr/bin/env node
// SPDX-License-Identifier: MPL-2.0

"use strict";
// Exercise the shipped page/Worker code, not a second renderer implementation.
// Usage: node scripts/tests/test-browser-preview.cjs [--wasm PATH]
// Build browser assets first to include the real WASM render/packaging checks.
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const assert = require("node:assert/strict");
const {test} = require("node:test");

const root = path.resolve(__dirname, "../..");
const args = process.argv.slice(2);
if (args.length && (args.length !== 2 || args[0] !== "--wasm")) throw new Error("Usage: test-browser-preview.cjs [--wasm PATH]");
const wasmPath = path.resolve(args[1] || process.env.NOTE4C_PREVIEW_WASM || path.join(root, "target/web-assets/raw/preview.wasm"));
const management = fs.readFileSync(path.join(root, "app/os/src/server.html"), "utf8");
const editor = fs.readFileSync(path.join(root, "app/os/src/editor/editor.html"), "utf8");
const workerSource = fs.readFileSync(path.join(root, "app/os/src/editor/preview-worker.js"), "utf8");
const bundleId = "a".repeat(64);
const tick = () => new Promise(resolve => setImmediate(resolve));

function between(source, start, end) {
  const begin = source.indexOf(start), finish = source.indexOf(end, begin);
  assert(begin >= 0 && finish > begin, "Production function boundaries must remain identifiable");
  return source.slice(begin, finish);
}

function elements() {
  const values = new Map();
  return id => {
    if (!values.has(id)) values.set(id, {
      value: "", files: [], textContent: "", hidden: false, disabled: false, children: [],
      replaceChildren(...children) { this.children = children; },
      append(...children) { this.children.push(...children); }
    });
    return values.get(id);
  };
}

const archiveManifest = {
  schema_version: 1, api_version: 1,
  files: [
    {path: "/examples.json", content_type: "application/json", encoding: "gzip", size: 90000, sha256: "b".repeat(64)},
    {path: "/preview.wasm", content_type: "application/wasm", encoding: "gzip", size: 50000, sha256: "c".repeat(64)}
  ]
};

function archive(manifest = archiveManifest, trailingBytes = 0) {
  const bytes = new TextEncoder().encode(JSON.stringify(manifest)), header = new Uint8Array(8);
  header.set([78, 52, 65, 49]);
  new DataView(header.buffer).setUint32(4, bytes.length, true);
  const file = new Blob([header, bytes, ...manifest.files.map(file => new Uint8Array(file.size)), new Uint8Array(trailingBytes)]);
  file.name = "preview.pna";
  return file;
}

function uploadHarness({cancelChunk = false, failChunk = false, badProgress = false, installedManifest = null} = {}) {
  const dom = elements(), calls = [];
  dom("token").value = "test-token";
  let manifest, received = [], context;
  const uploadStatus = () => ({bundle_id: bundleId, files: manifest.files.map((file, index) => ({
    path: file.path, size: file.size, received: received[index], complete: received[index] === file.size
  }))});
  const fetch = async (url, options) => {
    calls.push({url, options});
    assert.equal(options.headers["X-Note4c-Token"], "test-token");
    if (url.endsWith("/status")) return {ok: true, json: async () => ({
      api_version: 1,
      storage: {total_bytes: 8388608, used_bytes: 1048576, free_bytes: 7340032, safe_available_bytes: 5242880},
      web: {active: {bundle_id: bundleId}, previous: null, installed: installedManifest ? [{api_version: installedManifest.api_version, active: {bundle_id: bundleId, manifest: installedManifest}}] : [], pending: null, issues: []}
    })};
    if (url.endsWith("/begin")) {
      manifest = JSON.parse(options.body);
      // Device reports a deliberately unaligned persisted offset. The client
      // must use this offset, not restart at zero or round it to a chunk size.
      received = manifest.files.map((file, index) => index ? 0 : Math.min(file.size, 12345));
    }
    if (url.includes("/chunk?")) {
      const query = new URL("http://device" + url).searchParams;
      const index = Number(query.get("file")), offset = Number(query.get("offset"));
      assert.equal(query.get("bundle"), bundleId);
      assert.equal(offset, received[index]);
      assert(options.body instanceof Blob);
      assert(options.body.size > 0 && options.body.size <= 32768);
      if (cancelChunk) {
        vm.runInContext("previewAssetsController.abort()", context);
        throw new DOMException("cancel", "AbortError");
      }
      if (failChunk) return {ok: false, status: 400, json: async () => ({error: {message: "hash mismatch"}})};
      received[index] += options.body.size - (badProgress ? 1 : 0);
    }
    if (url.endsWith("/commit")) return {ok: true, json: async () => ({bundle_id: bundleId, manifest})};
    return {ok: true, json: async () => uploadStatus()};
  };
  context = vm.createContext({
    Blob, TextEncoder, TextDecoder, DataView, Uint8Array, DOMException, AbortController,
    setTimeout, clearTimeout, $: dom, node: (tag, className, text) => ({tag, className, text}), fetch,
    statusError: (data, status) => new Error(data.error?.message || String(status))
  });
  vm.runInContext(between(management, "let previewAssetsStatus=", "async function navigate(") +
    "\nglobalThis.test = {readPreviewArchive, checkedUploadStatus, installPreviewAssets};", context);
  dom("previewAssetsFile").files = [archive()];
  return {dom, calls, api: context.test};
}

function workerHarness() {
  const workers = [], timers = new Map();
  let assetRequests = 0, timerId = 0;
  class Worker {
    constructor(url) { assert.equal(url, "/editor-worker.js"); this.terminated = false; workers.push(this); }
    postMessage(message) { this.request = message; }
    terminate() { this.terminated = true; }
    reply(value) { this.onmessage({data: {id: this.request.id, revision: this.request.revision, ...value}}); }
  }
  const context = vm.createContext({
    TextEncoder, Uint8Array, ArrayBuffer, Worker, DOMException, Error,
    setTimeout(callback, delay) { const id = ++timerId; timers.set(id, {callback, delay}); return id; },
    clearTimeout(id) { timers.delete(id); },
    browserAssets: async () => { assetRequests++; return {bundle_id: bundleId}; }
  });
  vm.runInContext("const encoder = new TextEncoder(), workerOperations = new Set(); let workerSequence = 0, revision = 1;" +
    between(editor, "async function browserCall(", "function checkedResult(") + "\nglobalThis.call = browserCall;", context);
  return {
    call: context.call, workers, timers, get assetRequests() { return assetRequests; },
    onlyTimer() { assert.equal(timers.size, 1); return [...timers.values()][0]; }
  };
}

test("management, editor and Worker sources are valid JavaScript", () => {
  for (const [filename, source] of [["management", management], ["editor", editor]]) {
    const script = source.match(/<script>([\s\S]*?)<\/script>/);
    assert(script);
    new vm.Script(script[1], {filename});
  }
  new vm.Script(workerSource, {filename: "preview-worker.js"});
});

test("management starter keeps its complete MIT notice when exported", () => {
  const initial = management.match(/<textarea id="source" class="source">([\s\S]*?)<\/textarea>/)[1];
  const reset = JSON.parse(management.match(/const newWidgetSource = ("(?:[^"\\]|\\.)*");/)[1]);
  const notice = fs.readFileSync(path.join(root, "examples/widgets/LICENSE"), "utf8").trimEnd();
  assert.equal(reset, initial);
  assert(initial.startsWith("// SPDX-License-Identifier: MIT\n"));
  assert(initial.includes(notice));
  assert(Buffer.byteLength(initial) <= 5120);
  const pkg = {manifest: {id: "sample.hello"}, source: initial, resources: []};
  const context = vm.createContext({TextEncoder, packageFromEditor: () => pkg});
  vm.runInContext(between(management, "const encoder =", "function checkedPackage()") +
    "\nglobalThis.exported = pnwFromEditor();", context);
  assert(Buffer.from(context.exported.bytes).includes(Buffer.from(notice)));
});

test("widget exports use .pnw in management and both Studio engines", async () => {
  const bytes = new Uint8Array([80, 75, 3, 4]), pkg = {manifest: {id: "sample.test"}};
  const dom = elements(), downloads = [];
  const context = vm.createContext({
    $: dom, Blob,
    checkedPackage: () => ({bytes, pkg}),
    URL: {createObjectURL: () => "blob:test", revokeObjectURL() {}},
    document: {createElement: () => ({click() { downloads.push(this.download); }})},
    setTimeout() {}, feedback() {}
  });
  vm.runInContext(between(management, '$("exportWidget").onclick=', '$("openLocalEditor").onclick='), context);
  dom("exportWidget").onclick();
  assert.deepEqual(downloads, ["sample.test.pnw"]);
  for (const wasmPreview of [false, true]) {
    const dom = elements(), downloads = [];
    const context = vm.createContext({
      $: dom, Blob, Uint8Array, wasmPreview, revision: 1, dirty: true,
      candidate: () => pkg, checkedResult: result => result,
      browserCall: async () => ({bytes: [...bytes]}),
      fetch: async () => ({ok: true, blob: async () => new Blob([bytes])}),
      safeName: value => value, packageStatus() {},
      download: (blob, name) => downloads.push({blob, name})
    });
    vm.runInContext(between(editor, '$("exportPackage").onclick =', '$("exportPng").onclick ='), context);
    await dom("exportPackage").onclick();
    assert.equal(downloads.length, 1);
    assert.equal(downloads[0].name, "sample.test.pnw");
    assert.deepEqual(new Uint8Array(await downloads[0].blob.arrayBuffer()), bytes);
    assert.equal(dom("exportPackage").disabled, false);
  }
});

test("new and legacy resource extensions reach the same validated upload flow", async () => {
  for (const name of ["preview.pna", "preview.n4a", "PREVIEW.PNA", "PREVIEW.N4A"]) {
    const {api, calls, dom} = uploadHarness();
    const file = archive(); file.name = name;
    dom("previewAssetsFile").files = [file];
    await api.installPreviewAssets();
    assert.equal(calls.filter(call => call.url.endsWith("/commit")).length, 1, name);
  }
  const {api, calls} = uploadHarness();
  const wrong = archive(); wrong.name = "preview.zip";
  await assert.rejects(() => api.readPreviewArchive(wrong), /\.pna/);
  assert.equal(calls.length, 0);
});

test("archive parser rejects malformed schema, order, paths, encodings, hashes and lengths", async () => {
  const {api} = uploadHarness();
  assert.equal((await api.readPreviewArchive(archive())).total, 140000);
  for (const change of [
    value => value.schema_version = 2,
    value => value.api_version = 4294967296,
    value => value.files.reverse(),
    value => value.files[0].path = "/../outside",
    value => value.files[0].path = "/a//b",
    value => value.files[0].sha256 = "bad",
    value => value.files[0].encoding = "br",
    value => value.files[0].content_type = "a".repeat(97)
  ]) {
    const invalid = structuredClone(archiveManifest); change(invalid);
    await assert.rejects(() => api.readPreviewArchive(archive(invalid)));
  }
  await assert.rejects(() => api.readPreviewArchive(archive(archiveManifest, 1)));
  assert.throws(() => api.checkedUploadStatus({bundle_id: bundleId, files: []}, archiveManifest));
  const boundary = structuredClone(archiveManifest);
  boundary.api_version = 4294967295;
  boundary.files[0].content_type = "a".repeat(96);
  boundary.files[0].size = 0;
  boundary.files[0].sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
  assert.equal((await api.readPreviewArchive(archive(boundary))).total, 50000);
});

test("resource UI skips every write for an identical active manifest, including a future API", async () => {
  for (const apiVersion of [1, 2]) {
    const installedManifest = structuredClone(archiveManifest);
    installedManifest.api_version = apiVersion;
    const {api, calls, dom} = uploadHarness({installedManifest});
    dom("previewAssetsFile").files = [archive(installedManifest)];
    await api.installPreviewAssets();
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, "/web/v1/status");
    assert.equal(calls[0].options.method, "GET");
    assert.equal(dom("previewAssetsProgress").value, dom("previewAssetsProgress").max);
    assert.equal(dom("installPreviewAssets").disabled, false);
    assert.match(dom("previewAssetsFeedback").textContent, /已安装.*无需重复上传/);
  }
});

test("a changed file SHA still installs even if the reported bundle ID is unchanged", async () => {
  const installedManifest = structuredClone(archiveManifest);
  installedManifest.files[0].sha256 = "d".repeat(64);
  const {api, calls, dom} = uploadHarness({installedManifest});
  await api.installPreviewAssets();
  assert.equal(calls[0].url, "/web/v1/status");
  assert.equal(calls.filter(call => call.url.endsWith("/begin")).length, 1);
  assert(calls.some(call => call.url.includes("/chunk?")));
  assert.equal(calls.filter(call => call.url.endsWith("/commit")).length, 1);
  assert.match(dom("previewAssetsFeedback").textContent, /已安装/);
});

test("an empty resource follows the store contract without sending an empty chunk", async () => {
  const manifest = {schema_version: 1, api_version: 1, files: [{
    path: "/empty.txt", content_type: "text/plain", encoding: "identity", size: 0,
    sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
  }]};
  const {api, calls, dom} = uploadHarness();
  dom("previewAssetsFile").files = [archive(manifest)];
  await api.installPreviewAssets();
  assert.equal(calls.filter(call => call.url.includes("/chunk?")).length, 0);
  assert.equal(calls.filter(call => call.url.endsWith("/commit")).length, 1);
  assert.equal(dom("previewAssetsProgress").value, dom("previewAssetsProgress").max);
  assert.match(dom("previewAssetsFeedback").textContent, /已安装/);
});

test("resource UI resumes exact offsets, sends bounded Blob chunks and commits only after completion", async () => {
  const {api, calls, dom} = uploadHarness();
  await api.installPreviewAssets();
  assert.equal(calls.filter(call => call.url.endsWith("/commit")).length, 1);
  assert.equal(calls.find(call => call.url.includes("/chunk?")).url,
    `/web/v1/chunk?bundle=${bundleId}&file=0&offset=12345`);
  assert.equal(dom("previewAssetsProgress").value, 140000);
  assert.equal(dom("installPreviewAssets").disabled, false);
  assert.match(dom("previewAssetsFeedback").textContent, /已安装/);
});

test("resource UI does not commit cancelled, failed or inconsistent uploads and restores controls", async () => {
  for (const options of [{cancelChunk: true}, {failChunk: true}, {badProgress: true}]) {
    const {api, calls, dom} = uploadHarness(options);
    await api.installPreviewAssets();
    assert.equal(calls.filter(call => call.url.endsWith("/commit")).length, 0);
    assert.equal(dom("installPreviewAssets").disabled, false);
    assert.equal(dom("cancelPreviewAssets").hidden, true);
    assert.match(dom("previewAssetsFeedback").textContent, options.cancelChunk ? /已取消/ : options.failChunk ? /hash mismatch/ : /不一致/);
  }
});

test("resource UI rejects invalid archives before sending anything", async () => {
  const {api, calls, dom} = uploadHarness();
  dom("previewAssetsFile").files = [archive(archiveManifest, 1)];
  await api.installPreviewAssets();
  assert.equal(calls.length, 0);
  assert.equal(dom("installPreviewAssets").disabled, false);
});

test("Worker download timeout is 45 seconds and terminates the request", async () => {
  const h = workerHarness(), promise = h.call(0, {});
  await tick();
  assert.equal(h.onlyTimer().delay, 45000);
  h.onlyTimer().callback();
  await assert.rejects(promise, /下载超过 45 秒/);
  assert(h.workers[0].terminated);
  assert.equal(h.timers.size, 0);
});

test("execute phase starts one 10-second deadline, ignoring stale and duplicate phases", async () => {
  const h = workerHarness(), promise = h.call(0, {}, undefined, 12);
  await tick();
  const worker = h.workers[0], downloadTimer = h.onlyTimer();
  worker.reply({revision: 11, phase: "execute"});
  worker.reply({id: worker.request.id + 1, phase: "execute"});
  assert.equal(h.onlyTimer(), downloadTimer);
  worker.reply({phase: "execute"});
  const executionTimer = h.onlyTimer();
  assert.equal(executionTimer.delay, 10000);
  assert.notEqual(executionTimer, downloadTimer);
  worker.reply({phase: "execute"});
  assert.equal(h.onlyTimer(), executionTimer);
  executionTimer.callback();
  await assert.rejects(promise, /执行超过 10 秒/);
  assert(worker.terminated);
  worker.reply({phase: "execute"});
  assert.equal(h.timers.size, 0, "a queued phase after termination must not start a new timer");
});

test("Worker cancellation in either phase terminates and clears its timer", async () => {
  for (const executing of [false, true]) {
    const h = workerHarness(), abort = new AbortController(), promise = h.call(0, {}, abort.signal);
    await tick();
    if (executing) h.workers[0].reply({phase: "execute"});
    abort.abort();
    await assert.rejects(promise, error => error.name === "AbortError");
    assert(h.workers[0].terminated);
    assert.equal(h.timers.size, 0);
  }
});

test("Worker results ignore old revisions, and both success and errors terminate", async () => {
  const h = workerHarness(), promise = h.call(0, {}, undefined, 12);
  await tick();
  const worker = h.workers[0]; let resolved = false;
  promise.then(() => resolved = true);
  worker.reply({phase: "execute"});
  worker.reply({revision: 11, result: {ok: true}});
  await tick(); assert.equal(resolved, false);
  worker.reply({result: {ok: true}});
  assert.equal((await promise).ok, true);
  assert(worker.terminated); assert.equal(h.timers.size, 0);
  const failed = h.call(0, {});
  await tick();
  h.workers[1].reply({error: "trap recovered"});
  await assert.rejects(failed, /trap recovered/);
  assert(h.workers[1].terminated); assert.equal(h.timers.size, 0);
  const requests = h.assetRequests;
  await assert.rejects(() => h.call(0, {source: "x".repeat(65536)}), /64 KiB/);
  assert.equal(h.assetRequests, requests, "oversized input is rejected before fetching resources");
});

test("overall render deadline allows 70 seconds for WASM and retains 30 seconds for native", async () => {
  for (const wasmPreview of [true, false]) {
    const dom = elements(), timers = new Map(); let timerId = 0;
    dom("previewSurface").classList = {toggle() {}, remove() {}};
    const pending = signal => new Promise((resolve, reject) => signal.addEventListener("abort", () => reject(new DOMException("timeout", "AbortError")), {once: true}));
    const context = vm.createContext({
      $: dom, wasmPreview, AbortController,
      updateEventList() {}, updateControls() {}, status() {}, previewRequest: () => ({}), responseJson: async value => value,
      browserCall: (op, body, signal) => pending(signal),
      fetch: (url, options) => pending(options.signal),
      setTimeout(callback, delay) { const id = ++timerId; timers.set(id, {callback, delay}); return id; },
      clearTimeout(id) { timers.delete(id); }
    });
    vm.runInContext("let controller = null, requestSequence = 0, revision = 0, inputEvents = [], currentPreview = false, busy = false, hasFrame = false;" +
      between(editor, "async function render(", "function addResource(") + "\nglobalThis.render = render;", context);
    const promise = context.render([]);
    assert.equal(timers.size, 1);
    const timer = [...timers.values()][0];
    assert.equal(timer.delay, wasmPreview ? 70000 : 30000);
    timer.callback();
    await promise;
    assert.equal(timers.size, 0);
    assert.equal(dom("frameState").textContent, "运行失败");
  }
});

async function actualWorker(op, input, moduleBytes) {
  const replies = [], fetches = [];
  let downloaded = false, compiled = false;
  const actualWebAssembly = {
    ...WebAssembly, Module: WebAssembly.Module, Memory: WebAssembly.Memory, RuntimeError: WebAssembly.RuntimeError,
    compile: async bytes => {
      assert(downloaded);
      assert.equal(replies.at(-1)?.phase, "execute", "execution phase must precede compilation");
      compiled = true;
      return WebAssembly.compile(bytes);
    },
    instantiate: (...args) => WebAssembly.instantiate(...args)
  };
  const context = vm.createContext({
    TextDecoder, Uint8Array, WebAssembly: actualWebAssembly, performance,
    self: {postMessage: reply => replies.push(reply)},
    fetch: async url => {
      fetches.push(url);
      assert.equal(replies.length, 0, "execute phase must wait for download completion");
      return {ok: true, arrayBuffer: async () => {
        downloaded = true;
        return moduleBytes.buffer.slice(moduleBytes.byteOffset, moduleBytes.byteOffset + moduleBytes.byteLength);
      }};
    }
  });
  vm.runInContext(workerSource, context);
  await context.self.onmessage({data: {id: 3, revision: 7, bundle: bundleId, op,
    bytes: input instanceof Uint8Array ? input : new TextEncoder().encode(JSON.stringify(input))}});
  assert(compiled);
  assert(fetches.every(url => url === `/web/${bundleId}/preview.wasm`));
  assert.equal(replies.length, 2);
  for (const reply of replies) { assert.equal(reply.id, 3); assert.equal(reply.revision, 7); }
  assert(!replies[1].error, replies[1].error);
  return replies[1].result;
}

test("shipped Worker executes real WASM: counter frame, input replay, packaging and recovery", {
  skip: fs.existsSync(wasmPath) ? false : "Build browser assets or pass --wasm to verify the real renderer"
}, async () => {
  const moduleBytes = fs.readFileSync(wasmPath);
  const pkg = {
    manifest: JSON.parse(fs.readFileSync(path.join(root, "examples/widgets/counter/manifest.json"), "utf8")),
    source: fs.readFileSync(path.join(root, "examples/widgets/counter/main.rn"), "utf8"), resources: []
  };
  const request = {package: pkg, config: {}, environment: {unix_ms: Date.now(), utc_offset_minutes: 480, http: [], shared_files: {}}, events: []};
  const baseline = await actualWorker(0, request, moduleBytes);
  assert(baseline.ok, JSON.stringify(baseline.error)); assert.equal(baseline.pixels.length, 30000);
  const changed = await actualWorker(0, {...request, events: [{kind: "click", key: "boot", advance_ms: 1000}]}, moduleBytes);
  assert(changed.ok); assert.notDeepEqual(Buffer.from(changed.pixels), Buffer.from(baseline.pixels));
  const repeated = await actualWorker(0, request, moduleBytes);
  assert.deepEqual(Buffer.from(repeated.pixels), Buffer.from(baseline.pixels), "each preview starts from the same initial state");
  const encoded = await actualWorker(1, pkg, moduleBytes); assert(encoded.ok);
  const decoded = await actualWorker(2, new Uint8Array(encoded.bytes), moduleBytes);
  assert(decoded.ok); assert.equal(decoded.package.source, pkg.source);
  const malformed = await actualWorker(0, {package: {}, events: []}, moduleBytes);
  assert.equal(malformed.ok, false);
  const recovered = await actualWorker(0, request, moduleBytes);
  assert(recovered.ok); assert.deepEqual(Buffer.from(recovered.pixels), Buffer.from(baseline.pixels));
});
