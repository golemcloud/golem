const demo = await AgentRegistryDemo.get("query-test");
const result = await demo.exploreAgentQuery();
console.log(result);
