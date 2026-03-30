const agent = PrincipalAgent.get("test-agent");
const created = await agent.whoCreated();
console.log(created);
