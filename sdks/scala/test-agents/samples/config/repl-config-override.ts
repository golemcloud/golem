const caller = ConfigCallerAgent.get("test-override");
const result = await caller.callWithOverride();
console.log(`config-result=${result}`);
