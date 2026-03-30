const c = Coordinator.newPhantom("demo2");
const r = await c.route("demo2", 42, "hello");

const p = {
  name: "abc",
  count: 7,
  note: "n",
  flags: ["x", "y", "z"],
  nested: { x: 1.5, tags: ["a", "b"] },
};

const t = await c.routeTyped("demo2", 42, p);
console.log({ r, t });
