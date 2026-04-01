const w = await ApprovalWorkflow.get("demo");
const h = await Human.get("demo");

const started = await w.begin();
const decided = await h.decide("demo", "approved");
const outcome = await w.awaitOutcome();

console.log({ started, decided, outcome });
