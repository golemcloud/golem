const c = await AutoSnapshotCounter.upsert("auto-demo");
const a = await c.increment();
const b = await c.increment();
const d = await c.increment();
console.log({ a, b, d });
