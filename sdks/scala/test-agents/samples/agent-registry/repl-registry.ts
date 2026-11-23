const demo = await AgentRegistryDemo.upsert("registry-test");
const result = await demo.exploreRegistry();
console.log(result);
