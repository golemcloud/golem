const demo = await ObservabilityDemo.get("demo");
const result = await demo.durabilityDemo();
console.log(result);
