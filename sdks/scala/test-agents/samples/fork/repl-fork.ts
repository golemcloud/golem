const demo = await ForkDemo.upsert("demo");
const result = await demo.runFork();
console.log(result);
