const demo = await ObservabilityDemo.get("demo");
const trace = await demo.traceDemo();
console.log(trace);
