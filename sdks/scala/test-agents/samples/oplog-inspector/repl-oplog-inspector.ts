const inspector = await OplogInspector.get("demo");
const recent = await inspector.inspectRecent();
console.log(recent);
