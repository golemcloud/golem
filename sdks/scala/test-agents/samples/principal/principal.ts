const agent = await PrincipalAgent.upsert("test-agent");
const created = await agent.whoCreated();
console.log(created);
