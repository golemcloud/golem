const agent = SyncReturnAgent.get();
const greeting = await agent.greet("world");
const sum = await agent.add(3, 4);
await agent.touch("test-tag");
const tag = await agent.lastTag();
console.log(`greeting=${greeting} sum=${sum} tag=${tag}`);
