# Worker lifecycle

Rules for code under `worker/`. The `understanding-durable-execution` skill explains the
reconstruction path, invocation timelines and crash windows these rules protect.

## One reconstruction path

Suspend, interrupt, `Restart` (simulated crash), eviction, resharding and process loss all end
in the same place: the outer loop of `invocation_loop.rs::run` creates a new `Store`, runs
`prepare_instance`, replays, and publishes Live. Do not add a second recovery mechanism or a
lifecycle branch that assumes the previous `Store` still exists. Component revision, plugins and
metadata are fixed at instance creation; changing them means going through the outer loop, not
rewinding a cursor.

## Invocation acceptance and results

- An invocation is accepted only after its `PendingAgentInvocation` entry is committed
  (`enqueue_worker_invocation_with_effect`). Never report acceptance to a caller — local or
  remote — before that commit.
- Dedupe by idempotency key before appending (`lookup_invocation_result` ≠ `LookupResult::New`
  returns the existing invocation). Same key = same invocation, never new work.
- `AgentInvocationFinished` is committed with `CommitLevel::Always` before waiters are notified
  (`on_agent_invocation_success`; failures go through `on_invocation_failure`); keep that
  ordering. No positional durable-call `Start`/`End` may be appended for an invocation after its
  `Finished` entry (`tail_work.rs` enforces this); hint entries such as streaming-session
  completion legitimately follow it.
- Status (`status.rs`) is folded from the oplog; caches and checkpoints are baselines, never a
  source of truth. Checkpoint only at clean boundaries.

## Eviction and retry

`EvictionClass` must never unload an actively executing worker or one holding non-durable
in-memory work. `RetryDecision` decides *when* to reconstruct, not *whether* state survives — a
scheduling-triggered unload must not become a guest-visible timeout, cancellation, failed
invocation, retry-budget charge or new idempotency key.

## Ephemeral agents

Ephemeral agents are fail-stop. `reconstructed_ephemeral` exists for observation and invocation
result lookup; the instance must never be started again (`INACTIVE_EPHEMERAL_AGENT_ERROR`).

## Entity scheduling

`OwnerExecution` (`instance.rs`) is the single durable execution stream for an owner and all of
its entity Stores; `HostedInstance::invoke_scoped` runs one entity export and destroys its Store.
`EntitySlot` (`entity_slot.rs`) only registers active invocations and never grants execution;
`OwnerLane` (`owner_lane.rs`) alone serializes filesystem-capable bodies and returns
`WouldDeadlock` for a synchronous call back into a blocked owner. Do not add a second scheduler
or move admission into the slot.

## Cut points

Revert and fork split the oplog at a cut point. `cut_point.rs` rejects cuts that separate a
`Start` from its terminal, an `End` from its `CompletionDelivered`/`CompletionDiscarded` marker,
or split an atomic region or remote transaction. Any new paired durable construct must be added
to that validation.
