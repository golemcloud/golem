import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import { loadFixture, toolInput } from "./check.mjs";

const [path] = process.argv.slice(2);
assert(path, "usage: node bench.mjs <tool-or-mixed-main.js>");
const start = performance.now();
const { linked } = await loadFixture(path);
const loadMs = performance.now() - start;
const input = toolInput("x".repeat(65536));
const invoke = async () => {
  const result = await linked.golemTool010Guest.invoke(
    "echo",
    [],
    input,
    undefined,
    undefined,
    { tag: "anonymous" },
  );
  assert.equal(
    result.result.value.valueNodes[result.result.value.root].val,
    `echo:${"x".repeat(65536)}`,
  );
};
for (let i = 0; i < 100; i++) await invoke();
const samples = [];
for (let i = 0; i < 1000; i++) {
  const before = performance.now();
  await invoke();
  samples.push(performance.now() - before);
}
samples.sort((a, b) => a - b);
console.log(
  JSON.stringify({
    runtime: process.version,
    path,
    loadMs,
    payloadBytes: 65536,
    warmup: 100,
    iterations: samples.length,
    medianMs: samples[500],
    p95Ms: samples[950],
  }),
);
