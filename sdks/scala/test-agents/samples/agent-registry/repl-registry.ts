const demo = await AgentRegistryDemo.get("registry-test");
const result = await demo.exploreRegistry();
console.log(result);
