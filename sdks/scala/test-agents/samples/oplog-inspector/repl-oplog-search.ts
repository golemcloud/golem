const inspector = OplogInspector.get("demo");
const results = await inspector.searchOplog("increment");
console.log(results);
