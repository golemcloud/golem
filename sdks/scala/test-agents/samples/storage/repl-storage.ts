const demo = await StorageDemo.get("demo");
const config = await demo.configDemo();
console.log(config);
