const demo = await AgentRegistryDemo.get("phantom-test");
const result = await demo.phantomDemo();
console.log(result);
