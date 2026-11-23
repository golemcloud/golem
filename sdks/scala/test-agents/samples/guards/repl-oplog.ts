const demo = await GuardsDemo.upsert("oplog-test");
const result = await demo.oplogDemo();
console.log(result);
