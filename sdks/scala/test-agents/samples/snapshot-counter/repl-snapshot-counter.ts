const c = await SnapshotCounter.upsert("custom-demo");
const a = await c.increment();
const b = await c.increment();
const d = await c.increment();
console.log({ a, b, d });
