// SPDX-License-Identifier: MPL-2.0

"use strict";

// Each request owns a Worker. The page terminates it after completion or timeout,
// including traps, so a failed instance cannot affect the next preview.
self.onmessage = async event => {
  const {id, revision, bundle, op, bytes} = event.data || {};
  try {
    if (typeof bundle !== "string" || !/^[a-f0-9]{64}$/.test(bundle) || ![0, 1, 2].includes(op) || !(bytes instanceof Uint8Array) || bytes.byteLength > 65536) throw new Error("预览请求格式无效");
    const response = await fetch("/web/" + bundle + "/preview.wasm", {credentials:"same-origin"});
    if (!response.ok) throw new Error("预览资源读取失败，请到设备「文件与存储」重新安装预览资源包。");
    const moduleBytes = await response.arrayBuffer();
    self.postMessage({id, revision, phase:"execute"});
    const module = await WebAssembly.compile(moduleBytes);
    if (WebAssembly.Module.imports(module).some(value => value.module !== "env" || value.name !== "now_millis" || value.kind !== "function")) throw new Error("预览引擎接口与当前固件不兼容");
    const instance = await WebAssembly.instantiate(module, {env:{now_millis:() => performance.now()}});
    const wasm = instance.exports;
    if (!(wasm.memory instanceof WebAssembly.Memory) || ["alloc", "release", "call"].some(name => typeof wasm[name] !== "function")) throw new Error("预览引擎接口不完整");
    const input = wasm.alloc(bytes.byteLength) >>> 0;
    if (!input || input + bytes.byteLength > wasm.memory.buffer.byteLength) throw new Error("预览引擎无法分配输入内存");
    let output = 0, outputLength = 0;
    try {
      new Uint8Array(wasm.memory.buffer, input, bytes.byteLength).set(bytes);
      const descriptor = wasm.call(op, input, bytes.byteLength);
      if (typeof descriptor !== "bigint") throw new Error("当前浏览器不支持预览引擎接口");
      output = Number(descriptor & 0xffffffffn); outputLength = Number(descriptor >> 32n);
      if (!output || !outputLength || outputLength > 1048576 || output + outputLength > wasm.memory.buffer.byteLength) throw new Error("预览引擎返回数据无效");
      const result = JSON.parse(new TextDecoder("utf-8", {fatal:true}).decode(new Uint8Array(wasm.memory.buffer, output, outputLength)));
      self.postMessage({id, revision, result});
    } finally {
      if (output && outputLength) wasm.release(output, outputLength);
      wasm.release(input, bytes.byteLength);
    }
  } catch (error) {
    self.postMessage({id, revision, error:error instanceof WebAssembly.RuntimeError ? "预览引擎已停止；请检查脚本并重试。" : error.message || "浏览器预览失败"});
  }
};
