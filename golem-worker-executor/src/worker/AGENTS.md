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
- Dedupe by invocation ID before appending (`lookup_invocation_result` ≠ `LookupResult::New`
  returns the existing invocation). Same invocation ID = same invocation, never new work. The
  same raw idempotency key from another caller scope or for another operation is a different
  invocation.
- Successful completion appends `AgentInvocationFinished`, then waits for the state actor's commit
  receipt before notifying waiters (`on_agent_invocation_success`; failures go through
  `on_invocation_failure`). Durable agents use `CommitLevel::Always`, so the receipt follows
  storage. Ephemeral agents use `CommitLevel::Deferred`: the ordered writer handoff and receipt
  precede notification, without waiting for storage or the queued status fold (bounded writer
  backpressure may still wait). Do not weaken `Always` elsewhere; `DurableOnly` remains a no-op
  for ephemeral agents. Active durable-call work must settle before that invocation's `Finished`
  entry (`tail_work.rs` enforces this). A retained resource can span invocations; its deferred
  destructor may wait passively across historical finishes, then repair under its original Start
  when a later invocation drives the Store after replay becomes live. That repair stays active
  until its terminal is recorded. Hint entries such as streaming-session completion can follow
  `Finished` without another invocation.
- Reads that require the completion's fresh status must use the state actor's FIFO status read; it
  waits behind the queued fold. The separately persisted status cache remains asynchronous.
- Status (`status.rs`) is folded from the oplog; caches and checkpoints are baselines, never a
  source of truth. Checkpoint only at clean boundaries.

## Eviction and retry

`EvictionClass` must never unload an actively executing worker or one holding non-durable
in-memory work. `RetryDecision` decides *when* to reconstruct, not *whether* state survives — a
scheduling-triggered unload must not become a guest-visible timeout, cancellation, failed
invocation, retry-budget charge or new idempotency key.

## Ephemeral agents

Ephemeral agents are fail-stop. Before an ephemeral instance runs guest code,
`Worker::begin_ephemeral_execution` commits a `Restart` entry under the instance mutex; the status
fold sets `ephemeral_execution_begun` permanently. Once it is set, `reconstructed_ephemeral` exists
only for observation and invocation result lookup, and the instance must never be started again
(`INACTIVE_EPHEMERAL_AGENT_ERROR`). The lifecycle holds this as the `Inactive` phase, which a failed
ephemeral start also enters; a durable agent's failed start stays a retryable
`Unloaded { startup_failure }`. The one exception is an agent that never began and still holds
its constructor reservation (`never_started_ephemeral_admission`): it may accept one method when it
has none yet, as when a schedule repair finds the target its first attempt created, and it may run
one accepted scheduled method cold. A scheduled ephemeral target is created at schedule time with
its `PendingAgentInvocation` committed. Before the deadline its start is `NotDue`; after token
cancellation withdrew its work it is `Withdrawn`. Neither runs guest code, and both fail queued
inspection requests instead of keeping them for a later start. An inspection (filesystem or wallet)
is refused, not started, on an unloaded ephemeral agent that has not begun and has no due method
(`ephemeral_inspection_may_start`): it must not use up a target whose schedule is still being
repaired. An outside caller that knows the target's ID can still start it in that window (create
with wait, resume, update); the scheduling caller then traps with
`INACTIVE_EPHEMERAL_AGENT_ERROR`. A start that arrives while a `NotRunnable` start stops is
restarted after the stop if the work became due.

## Entity scheduling

`OwnerExecution` (`instance/mod.rs`) is the single durable execution stream for an owner and all of
its entity Stores; `HostedInstance::invoke_scoped` runs one entity export and destroys its Store.
`EntitySlot` (`entity_slot.rs`) only registers active invocations and never grants execution;
`OwnerLane` (`owner_lane.rs`) alone serializes filesystem-capable bodies and returns
`WouldDeadlock` for a synchronous call back into a blocked owner. Do not add a second scheduler
or move admission into the slot.

## Cut points

Revert and fork retain the exact inclusive oplog prefix at the cut point. A retained `Start`
without its terminal uses ordinary incomplete-call recovery. A retained accessor `End` without
its `CompletionDelivered`/`CompletionDiscarded` marker uses replay-tail delivery. Outcomes beyond
the cut do not constrain these calls. `cut_point.rs` still rejects cuts splitting a remote-transaction
outcome; audit new paired constructs against that validation.
