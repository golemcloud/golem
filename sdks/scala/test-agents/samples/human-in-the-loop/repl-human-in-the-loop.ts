const w = await ApprovalWorkflow.upsert("demo");
const h = await Human.upsert("demo");

const started = await w.begin();
const decided = await h.decide("demo", "approved");
const outcome = await w.awaitOutcome();

console.log({ started, decided, outcome });
