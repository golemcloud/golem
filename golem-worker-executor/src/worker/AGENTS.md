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
  for ephemeral agents. A successful constructor and agent methods that use no streams record
  their result at the export's return and notify waiters then; a constructor error drains first,
  within the invocation and tail-work time limits, and takes its fatal path. Tool children that already run, or need no admission,
  settle before the result is recorded, and their failure fails the invocation. The invocation's
  background work (spawned guest tasks, active store tasks, retained durable calls) drains after
  the result, without the invocation or tail-work time limits; capable tool children staged until
  their parent ends are admitted at the end of the drain (`end_tool_parent`) and settle before
  `AgentInvocationSettled`, the hint the end of the drain appends
  (`invocation.rs::record_at_return_and_drain`). The next invocation starts only after
  `Settled`. When a due queued invocation is the loop's next work, `Settled` is not committed on
  its own: the next `AgentInvocationStarted` commit stores both (`defer_settlement_commit`), and
  the loop commits a deferred `Settled` itself before other work, before it waits for a command
  or a permit, and when it exits (`commit_deferred_settlement`). The start of the next
  invocation (payload hydration, scope card, instance lock) runs with `Settled` buffered and the
  status `Running`. Methods that use streams (stream
  inputs or a stream result), snapshot save and load (including a manual update's),
  oplog-processor batches and external-tool invocations keep the settled path: they settle their
  active durable-call work before their terminal (`tail_work.rs`) and write no `Settled`. A
  failure during the drain never replaces the recorded result: a trap reconstructs under the retry
  policy, and an interrupt or suspension stops the agent until it resumes and reconstructs; giving
  up fails the agent and the queued invocations, an interrupt fails the queued invocations, and
  neither publishes a failure for the recorded one. A give-up during the drain records an
  exhausted retry state (`drain_give_up_retry_state`), and the result fold turns a successful
  result that settles into `InvocationEnd::BackgroundFailed` at that `Error` or at an `Exited`
  (`InvocationRequests::settling`): a same-key lookup returns the recorded result with its
  `background_failure`, and a same-key `Await` refused because an invocation give-up failed the
  agent or the agent exited still returns the recorded success (a recorded failure keeps the
  refusal; a recovery failure is not a background failure). A retained
  resource can span invocations; its deferred destructor may wait passively across historical terminals and settlements, then repair
  under its original Start when a later invocation drives the Store after replay becomes live.
  That repair stays active until its terminal is recorded. Hint entries such as streaming-session
  completion can follow `Finished` without another invocation.
- Reads that require the completion's fresh status must use the state actor's FIFO status read; it
  waits behind the queued fold. The separately persisted status cache remains asynchronous.
- Status (`status.rs`) is folded from a start status and the oplog entries after it. A fold uses
  the status cache as its start status when it has one; the cache is disposable. The clean
  checkpoint (`AgentStatusCheckpoint`) is the durable status baseline: no expiry, kept by an
  archive, removed only by agent deletion, and never read for another fingerprint. Checkpoint
  only at clean boundaries. A terminal with `awaits_settlement` keeps the agent `Running` until
  its `AgentInvocationSettled` (`AgentStatusRecord::settling_invocation`); lifecycle decisions
  that test for a running agent (interrupts, assignment recovery, retirement, the `Resumed`
  decision in `startup.rs`) must use the folded status, not the Store's record of the return.

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
outcome; audit new paired constructs against that validation. A cut may fall inside an
invocation's drain, between its terminal and its `AgentInvocationSettled`; replay then reaches the
replay tail with the anchor open and settles it live. A revert of the last N invocations is
resolved to a cut before it stops the agent; after the stop, `append_revert` recomputes the cut
from the stopped oplog, skipping regions that earlier reverts dropped, and rejects the revert as
stale unless it equals the authorized one. A repeated dispatch of a resolution that already
applied (the last `Revert` drops everything after the cut, and no invocation started after it)
succeeds without writing.
