const agent = await ConfigAgent.get("test");
const result = await agent.greet();
console.log(`config-default=${result}`);
