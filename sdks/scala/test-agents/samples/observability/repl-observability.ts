const demo = await ObservabilityDemo.upsert("demo");
const trace = await demo.traceDemo();
console.log(trace);
