const demo = await ObservabilityDemo.upsert("demo");
const result = await demo.durabilityDemo();
console.log(result);
