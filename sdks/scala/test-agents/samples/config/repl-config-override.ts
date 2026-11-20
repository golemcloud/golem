const caller = await ConfigCallerAgent.upsert("test-override");
const result = await caller.callWithOverride();
console.log(`config-result=${result}`);
