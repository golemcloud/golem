// Run against a prebuilt core module extracted with wasm-tools component unbundle.
// The harness never starts a compiler or a Golem process.
import assert from "node:assert/strict";
import fs from "node:fs";

const [path, capabilities = ""] = process.argv.slice(2);
const active = new Set(capabilities.split(","));
const module = new WebAssembly.Module(fs.readFileSync(path));
let instance;
let loadError;
const imports = {};
for (const entry of WebAssembly.Module.imports(module)) {
  assert.equal(entry.kind, "function");
  imports[entry.module] ??= {};
  imports[entry.module][entry.name] = (...args) => {
    if (entry.module === "wasi:random/insecure-seed@0.2.9" && entry.name === "insecure-seed") {
      const view = new DataView(instance.exports.memory.buffer);
      view.setBigUint64(args[0], 17n, true);
      view.setBigUint64(args[0] + 8, 43n, true);
      return;
    }
    if (entry.module === "[export]golem:api/load-snapshot@1.5.0" && entry.name === "[task-return]load") {
      assert.equal(args[0], 1);
      loadError = Buffer.from(instance.exports.memory.buffer, args[1], args[2]).toString("utf8");
      return;
    }
    throw new Error(`unexpected host call ${entry.module}#${entry.name}`);
  };
}
instance = new WebAssembly.Instance(module, imports);
const e = instance.exports;
const signature = (name, arity) => {
  assert.equal(typeof e[name], "function", name);
  assert.equal(e[name].length, arity, name);
};
const discover = (name, enabled, expectedName) => {
  signature(name, 0);
  signature(`cabi_post_${name}`, 1);
  const ptr = e[name]();
  const view = new DataView(e.memory.buffer);
  assert.equal(view.getUint8(ptr), 0);
  assert.equal(view.getUint32(ptr + 8, true), Number(enabled));
  if (enabled) {
    const record = view.getUint32(ptr + 4, true);
    // Tool names live on the root command, after the descriptor's version.
    const named = name === "golem:tool/guest@0.1.0#discover-tools" ? view.getUint32(record + 8, true) : record;
    assert.equal(Buffer.from(e.memory.buffer, view.getUint32(named, true), view.getUint32(named + 4, true)).toString("utf8"), expectedName);
  }
  e[`cabi_post_${name}`](ptr);
};
const discovery = () => {
  discover("golem:agent/guest@2.0.0#discover-agent-types", active.has("agent"), "MinimalAgent");
  discover("golem:tool/guest@0.1.0#discover-tools", active.has("tool"), "public-echo");
  discover("golem:tool/tool-middleware-guest@0.1.0#discover-tool-middlewares", active.has("middleware"), "minimal-policy");
};
discovery(); // Must run constructors before selecting the canonical entry point.

const lower = value => {
  const bytes = Buffer.from(value);
  const ptr = e.cabi_realloc(0, 0, 1, bytes.length);
  new Uint8Array(e.memory.buffer, ptr, bytes.length).set(bytes);
  return [ptr, bytes.length];
};
let warmMemory;
for (let round = 0; round < 30; round++) {
  for (const name of ["", "missing-é-🦀", "missing".repeat(16384)]) {
    for (const target of ["golem:tool/guest@0.1.0#get-tool", "golem:tool/tool-middleware-guest@0.1.0#get-tool-middleware"]) {
      signature(target, 2);
      signature(`cabi_post_${target}`, 1);
      const ptr = e[target](...lower(name));
      const view = new DataView(e.memory.buffer);
      assert.equal(view.getUint8(ptr), 1);
      assert.equal(view.getUint8(ptr + 4), 0);
      const len = view.getUint32(ptr + 12, true);
      assert.equal(len, Buffer.byteLength(name));
      assert.equal(Buffer.from(e.memory.buffer, view.getUint32(ptr + 8, true), len).toString("utf8"), name);
      e[`cabi_post_${target}`](ptr);
      discovery(); // Not-found scratch storage must not contaminate discovery.
    }
  }
  if (round === 0) warmMemory = e.memory.buffer.byteLength;
}
assert.equal(e.memory.buffer.byteLength, warmMemory, "post-return leaked a lowered name");
for (const [name, arity] of [
  ["golem:agent/guest@2.0.0#initialize", 1],
  ["golem:agent/guest@2.0.0#invoke", 1],
  ["golem:tool/guest@0.1.0#invoke", 1],
  ["golem:tool/tool-middleware-guest@0.1.0#invoke-tool-middleware", 1],
  ["golem:api/load-snapshot@1.5.0#load", 4],
  ["golem:api/save-snapshot@1.5.0#save", 0],
]) {
  signature(`[async-lift]${name}`, arity);
  signature(`[callback][async-lift]${name}`, 3);
}
if (!active.has("agent")) {
  for (const payload of ["", "asymmetric snapshot 🦀", "snapshot".repeat(16384)]) {
    loadError = undefined;
    const state = e["[async-lift]golem:api/load-snapshot@1.5.0#load"](...lower(payload), ...lower("not/a-snapshot"));
    assert.equal(state, 0);
    assert.equal(loadError, "component has no agent snapshot support");
  }
}
console.log(`raw ABI checks passed (${capabilities || "empty"}): discovery, UTF-8 ownership, retptr/post-return, async signatures and snapshot task-return`);
