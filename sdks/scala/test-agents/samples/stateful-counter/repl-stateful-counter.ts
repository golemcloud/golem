const c = await StatefulCounter.get(10);
const a = await c.increment();
const b = await c.increment();
const cur = await c.current();
console.log({ first: a, second: b, current: cur });
