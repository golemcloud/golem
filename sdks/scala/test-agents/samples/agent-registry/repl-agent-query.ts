const demo = await AgentRegistryDemo.upsert("query-test");
const result = await demo.exploreAgentQuery();
console.log(result);
