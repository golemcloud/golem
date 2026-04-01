const inspector = await OplogInspector.get("demo");
const results = await inspector.searchOplog("increment");
console.log(results);
