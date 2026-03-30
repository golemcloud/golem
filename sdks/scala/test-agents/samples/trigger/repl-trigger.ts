const t = TriggerTarget.get("demo1");
const a = await t.process(10, "hello");
const b = await t.process(32, "world");
const c = await t.ping();
console.log({ first: a, second: b, ping: c });
