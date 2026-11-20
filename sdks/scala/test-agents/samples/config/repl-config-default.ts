const agent = await ConfigAgent.upsert("test");
const result = await agent.greet();
console.log(`config-default=${result}`);
