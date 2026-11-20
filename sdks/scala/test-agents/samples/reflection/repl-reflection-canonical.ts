const caller = await ScalaToolReflectionCaller.upsert("canonical-json");
console.log(await caller.canonicalRoundTrip());
console.log(await caller.optionalRoundTrip());
