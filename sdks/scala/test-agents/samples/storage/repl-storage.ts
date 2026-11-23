const demo = await StorageDemo.upsert("demo");
const config = await demo.configDemo();
console.log(config);
