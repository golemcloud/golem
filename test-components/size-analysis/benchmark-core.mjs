// Isolated allocator/instantiation smoke benchmark, not a Golem latency benchmark.
// Usage: node benchmark-core.mjs report/core-0/module.wasm [other cores...]
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { performance } from "node:perf_hooks";

const samples = new Map();
const modules = process.argv.slice(2).map((path) => {
  const module = new WebAssembly.Module(readFileSync(path));
  const imports = {};
  for (const { module: namespace, name, kind } of WebAssembly.Module.imports(module)) {
    assert.equal(kind, "function", `unsupported import ${namespace}/${name}`);
    imports[namespace] ??= {};
    imports[namespace][name] = () => {
      throw new Error(`benchmark unexpectedly called ${namespace}/${name}`);
    };
  }
  samples.set(path, { instantiateMs: [], allocationMs: [] });
  return { path, module, imports };
});
assert.ok(modules.length, "pass at least one unbundled Rust core module");

for (let round = 0; round < 12; round++) {
  for (const { path, module, imports } of round % 2 ? [...modules].reverse() : modules) {
    const start = performance.now();
    const instance = new WebAssembly.Instance(module, imports);
    const instantiated = performance.now();
    const { cabi_realloc: realloc, memory } = instance.exports;
    assert.equal(typeof realloc, "function");
    for (let i = 0; i < 100000; i++) {
      const size = 17 + (i % 1024);
      const pointer = realloc(0, 0, 8, size);
      new Uint8Array(memory.buffer)[pointer] = 173;
      const grown = realloc(pointer, size, 8, size + 257);
      assert.equal(new Uint8Array(memory.buffer)[grown], 173);
      realloc(grown, size + 257, 8, 0);
    }
    const finished = performance.now();
    if (round >= 2) {
      samples.get(path).instantiateMs.push(instantiated - start);
      samples.get(path).allocationMs.push(finished - instantiated);
    }
  }
}
const median = (values) => [...values].sort((a, b) => a - b)[values.length / 2];
console.log(JSON.stringify({
  node: process.version,
  iterations: 100000,
  warmups: 2,
  samples: 10,
  results: [...samples].map(([path, times]) => ({
    path,
    instantiateMedianMs: median(times.instantiateMs),
    allocationMedianMs: median(times.allocationMs),
    ...times,
  })),
}, null, 2));
