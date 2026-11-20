const inspector = await OplogInspector.upsert("demo");
const results = await inspector.searchOplog("increment");
console.log(results);
