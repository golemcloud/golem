const inspector = await OplogInspector.upsert("demo");
const recent = await inspector.inspectRecent();
console.log(recent);
