// Core-ABI discovery/snapshot checks and warmed latency; no network or executor.
import fs from "node:fs";
import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";

const [wasm, expected, agents, tools] = process.argv.slice(2);
const module = new WebAssembly.Module(fs.readFileSync(wasm));
const exports = WebAssembly.Module.exports(module).map(e => e.name);
assert.deepEqual(exports.filter(e => e !== "memory" && e !== "_start").sort(), JSON.parse(expected).sort());
let instance;
let context = 0;
let snapshot;
let invocation;
const streamBytes = Uint8Array.from([0, 1, 63, 64, 65, 4096, 65537].flatMap(size => Array.from({length: size}, (_, i) => i % 251)));
let streamOffset = 0, streamWrites = 0, streamFinishes = 0, streamDrops = 0;
const imports = {};
for (const entry of WebAssembly.Module.imports(module)) {
  imports[entry.module] ??= {};
  assert.equal(entry.kind, "function");
  imports[entry.module][entry.name] = (...args) => {
    if (entry.name === "[context-get-0]") return context;
    if (entry.name === "[context-set-0]") { context = args[0]; return; }
    if (entry.name === "[waitable-set-new]") return 1;
    if (entry.name === "[waitable-set-drop]") return;
    if (entry.name === "[subtask-drop]") return;
    if (entry.module === "golem:tool/streams@0.1.0") {
      const view = new DataView(instance.exports.memory.buffer);
      if (entry.name === "[async-lower][method]tool-stdout-writer.write") {
        const bytes = new Uint8Array(instance.exports.memory.buffer, args[1], args[2]);
        assert.deepEqual(bytes, streamBytes.subarray(streamOffset, streamOffset + bytes.length));
        streamOffset += bytes.length;
        streamWrites++;
        view.setUint32(args[3], 0, true);
        return 2;
      }
      if (entry.name === "[async-lower][method]tool-stdout-writer.finish") {
        streamFinishes++;
        view.setUint32(args[1], 0, true);
        return 2;
      }
      if (entry.name === "[resource-drop]tool-stdout-writer") { streamDrops++; return; }
    }
    if (entry.name === "[task-return]save") {
      snapshot = {length: args[1], mime: Buffer.from(instance.exports.memory.buffer, args[2], args[3] * 2).toString("utf16le")};
      return;
    }
    if (entry.module === "[export]golem:tool/guest@0.1.0" && entry.name === "[task-return]invoke") {
      assert.equal(args[0], 0);
      assert.equal(args[1], 1);
      assert.equal(args[8], 1);
      const view = new DataView(instance.exports.memory.buffer);
      assert.equal(view.getUint8(args[7]), 12); // string-value
      const ptr = view.getUint32(args[7] + 8, true);
      const len = view.getUint32(args[7] + 12, true);
      invocation = Buffer.from(instance.exports.memory.buffer, ptr, len * 2).toString("utf16le");
      return;
    }
    throw new Error(`unexpected host call ${entry.module}#${entry.name}`);
  };
}
instance = new WebAssembly.Instance(module, imports);
const e = instance.exports;
e._start();
const discover = (name, count) => {
  const ptr = e[name]();
  const view = new DataView(e.memory.buffer);
  assert.equal(view.getUint8(ptr), 0);
  assert.equal(view.getUint32(ptr + 8, true), count);
  e[`cabi_post_${name}`](ptr);
};
const agentName = "golem:agent/guest@2.0.0#discover-agent-types";
const toolName = "golem:tool/guest@0.1.0#discover-tools";
discover(agentName, Number(agents));
discover(toolName, Number(tools));
discover("golem:tool/tool-middleware-guest@0.1.0#discover-tool-middlewares", 0);
let callback = e["[async-lift]golem:api/save-snapshot@1.5.0#save"]();
for (let step = 0; callback !== 0 && step < 100; step++) {
  assert.equal(callback & 15, 1);
  callback = e["[callback][async-lift]golem:api/save-snapshot@1.5.0#save"](0, 0, 0);
}
assert.equal(callback, 0);
assert.deepEqual(snapshot, {length: 0, mime: "application/octet-stream"});
const invoke = (commandName = "ping") => {
  const alloc = size => {
    const ptr = e.cabi_realloc(0, 0, 8, size);
    new Uint8Array(e.memory.buffer, ptr, size).fill(0);
    return ptr;
  };
  const string = value => {
    const bytes = Buffer.from(value, "utf16le");
    const ptr = alloc(bytes.length);
    new Uint8Array(e.memory.buffer, ptr, bytes.length).set(bytes);
    return ptr;
  };
  // Canonical memory layout of invoke's single indirect parameter record.
  const args = alloc(160), tool = string("probe"), command = string(commandName);
  const path = alloc(8), type = alloc(144), value = alloc(32);
  // The MoonBit lift consumes every list buffer, including empty lists.
  const emptyLists = Array.from({length: 5}, () => alloc(1));
  const view = new DataView(e.memory.buffer);
  const put = (ptr, value) => view.setUint32(ptr, value, true);
  put(args, tool); put(args + 4, 5);
  put(path, command); put(path + 4, commandName.length);
  put(args + 8, path); put(args + 12, 1);
  put(args + 16, type); put(args + 20, 1);
  put(type, 14); // empty record-type
  put(type + 8, emptyLists[0]);
  put(type + 100, emptyLists[1]);
  put(type + 108, emptyLists[2]);
  put(args + 24, emptyLists[3]);
  put(args + 36, value); put(args + 40, 1);
  put(value, 13); // empty record-value
  put(value + 8, emptyLists[4]);
  put(args + 64, 3); // anonymous principal
  if (commandName === "burst") {
    put(args + 56, 1); put(args + 60, 77);
    streamOffset = streamWrites = streamFinishes = streamDrops = 0;
  }
  invocation = undefined;
  let state = e["[async-lift]golem:tool/guest@0.1.0#invoke"](args);
  for (let step = 0; state !== 0 && step < 100; step++) {
    assert.equal(state & 15, 1);
    state = e["[callback][async-lift]golem:tool/guest@0.1.0#invoke"](0, 0, 0);
  }
  assert.equal(state, 0);
  e.cabi_realloc(args, 160, 8, 0);
  assert.equal(invocation, commandName === "ping" ? "pong" : "burst");
  if (commandName === "burst") {
    assert.equal(streamOffset, streamBytes.length);
    assert.equal(streamFinishes, 1);
    assert.equal(streamDrops, 1);
  }
};
const latency = {};
const operations = [[agentName, () => discover(agentName, Number(agents))], [toolName, () => discover(toolName, Number(tools))]];
if (Number(tools)) operations.push(["tool-invoke-ping", () => invoke()]);
for (const [name, operation] of operations) {
  for (let i = 0; i < 100; i++) operation();
  const samples = [];
  for (let sample = 0; sample < 7; sample++) {
    const start = performance.now();
    for (let i = 0; i < 1000; i++) operation();
    samples.push((performance.now() - start) * 1000 / 1000);
  }
  samples.sort((a,b) => a-b);
  latency[name] = samples[3];
}
let stream;
if (Number(tools)) {
  invoke("burst");
  const samples = [];
  for (let i = 0; i < 7; i++) {
    const start = performance.now();
    invoke("burst");
    samples.push((performance.now() - start) * 1000);
  }
  samples.sort((a,b) => a-b);
  stream = {bytes: streamOffset, writes: streamWrites, median_us: samples[3]};
}
process.stdout.write(JSON.stringify({exports: exports.length, warmed_median_us: latency, stream}));
