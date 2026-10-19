const caller = await ScalaToolReflectionCaller.get("canonical-json");
console.log(await caller.canonicalRoundTrip());
console.log(await caller.optionalRoundTrip());
