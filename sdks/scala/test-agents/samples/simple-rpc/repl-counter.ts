const c = await Coordinator.upsert("demo");
const a = await c.route("demo", 1, "hello");
const b = await c.route("demo", 2, "world");
console.log({ a, b });
