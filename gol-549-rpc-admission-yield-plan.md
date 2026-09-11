# GOL-549: release caller capacity during long RPC waits

This revised plan supersedes the earlier RPC phase-query/admission-ticket proposal.

## Problem and scope

A durable caller can hold the account's last concurrent-agent permit while waiting
for an RPC whose callee needs that permit. Neither invocation contains an infinite
loop: the caller waits for the result, and the callee waits for capacity. The
one-slot executor regression reproduces this capacity deadlock. Its connection to
the original reported incident remains unproven.

Sequential invocation processing and RPCs without deadlines are intentional.
An invocation that never finishes, or a logical invocation dependency cycle, is
not solved by changing admission. This change does not weaken either contract.

## Policy

Register pending durable RPC operations as suspendable waits. After 30 seconds
waiting for an RPC, check the same `safe_to_suspend` condition used by sleeps:
every live durable host call must have a suspendable wait, no durable scope may
be open, and no P3 HTTP request transmission may be pending. If unsafe work
prevents suspension, recheck every 10 seconds while the RPC remains pending.
Completing an HTTP request therefore permits a later suspension attempt; the
first failed check does not disarm the policy.

When eligible, persist a scheduled wakeup and return `InterruptKind::Suspend`.
The RPC wakeup is five seconds from the scheduling attempt. Suspension tears down
the instance and releases its permit; the scheduled wake automatically recovers
it through normal admission and oplog replay. Do not return a timeout error,
cancel the logical remote invocation, or generate a new idempotency key. This
does not use immediate Restart or OOM retry/backoff state. A replayed,
already-recorded result does not arm a timer.

Configuration: `suspend.rpc_suspend_after` defaults to 30 seconds and
`suspend.rpc_resume_after` defaults to five seconds. The existing
`suspend.wait_suspend_check_interval` controls rechecks (default ten seconds).
Five seconds avoids immediate replay/requeue churn while keeping recovery prompt;
it is a static initial policy, not an RPC deadline.

Example with one slot:

1. A runs and calls B. B's accepted invocation is durably queued.
2. B waits for capacity while A waits for B's result.
3. A's wait reaches 30 seconds; if safe, schedule its wake and suspend A.
4. B acquires capacity and finishes its queued invocation.
5. A's wake becomes due after five seconds. A reacquires capacity, recovers, and
   obtains B's result using the same call identity.

If A also has an unfinished HTTP request at 30 seconds, A keeps running. If that
request completes at 34 seconds, the next check around 40 seconds can suspend A
if its RPC is still pending. Admission and scheduler latency can add delay.

The same policy applies whether RPC dispatch is local or remote. It does not
query the target's phase, detect a dependency graph, exceed account limits, or
change Wasmtime. A long queue or slow target also justifies yielding; proof of
permit contention is unnecessary. Ephemeral callers do not opt into automatic
RPC suspension. Existing sleep safety exclusions remain intact.

Mixed waits choose the earliest real sleep deadline or `now + rpc_resume_after`,
regardless of which wait initiates suspension. After awaiting wakeup persistence,
recheck both eligibility and the registry's earliest required wakeup. A newly
registered RPC must not leave the agent parked until an older, hour-long sleep.
Capture the suspension timestamp before scheduling so a racing resume is not lost.

## Acceptance must survive loss of the caller

Target preparation establishes identity/metadata without requiring a running
instance or acquiring its execution permit. Public create/launch keeps its
existing wait-for-loading behavior.

For work requiring execution, the target owns the enqueue/commit/start prefix.
Acceptance is emitted only after durable queue commit. Losing the inbound request
must not abandon committed work before startup is requested. Reconnection uses
the existing invocation identity and idempotency handling.

Read-only cache hits remain immediate and persist nothing. Coalesced followers
share the executing owner's admission/completion; they do not create durable
aliases or additional queued invocations. Retained completion state wakes existing
followers even if the cache removes a failed producer.

## Implementation steps

1. Remove phase-query protocols, admission tickets, propagation, and unused
   scheduler-deferral machinery. Keep the required acceptance/cache corrections.
2. Share the sleep parking mechanism with synchronous RPC waits and async result
   waits, including redispatch during recovery. Make target preparation and
   synchronous streaming result attachment interruptible. Abandon/trap durable
   handles on lifecycle interruption rather than recording cancellation or failure.
3. Register async RPCs when their background task starts, even before guest `get`.
   Keep exactly one registration per pending operation. Remove it on completion
   and synchronously on cancellation before releasing the durable call's live
   permit. Result `get` drives suspension checks without registering again.
4. While an unloaded caller reacquires a permit, continue processing terminal
   lifecycle requests and channel closure. Preserve the pinned FIFO acquisition
   across ordinary wakeups. An ignored stale suspend must release its claimed
   terminal state so later restart requests remain effective.
5. Before checking suspension safety, synchronously release permits held only by
   successfully completed deferred completion markers. Preserve pending/failed
   markers and their errors for ordinary draining; both veto suspension. Keep all
   unrelated deferred events ordered. This lets a completed HTTP request stop
   blocking a parked RPC or sleep without introducing timer-driven oplog writes.
6. Review each implementation step with Oracle and run targeted regressions.

## Verification

- Two-slot control and one-slot RPC progress; a subsequent counter increment
  proves the RPC effect was applied exactly once.
- Interrupt and delete a recovering caller while another holder retains the sole
  permit. Assert termination/interrupted status before releasing that permit.
- Prepare an unloaded target under permit pressure; public create still waits.
- Read-only settled hits and coalesced followers persist no extra invocations;
  an owner survives cancellation of its initiating request.
- Reuse existing caller/callee and concurrent nested-stream recovery tests.
- Run targeted remote preparation/retry transport tests and scoped format/checks.
- Hold HTTP past 30 seconds, assert no suspension, release HTTP, then assert
  suspension and automatic RPC completion without repeating the HTTP request.
- Test registration cancellation/completion, repeated eligibility checks, mixed
  sleep/RPC wakeups, and registry/resume changes during scheduling.

## Tradeoffs and limits

Slow RPCs can cause repeated recovery and replay overhead. Thirty seconds is an
eligibility threshold, not an RPC deadline or a completion guarantee. Existing
lifecycle cleanup must finish before capacity is released. Scheduled wakeups can
race completion; ordinary recovery preserves the invocation's durable outcome.

Open durable scopes, unsafe sibling calls, and pending HTTP transmissions can
prevent suspension indefinitely. A completed but unconsumed async RPC can also
block eligibility until its durable handle finishes. These are conservative
limits of using the existing sleep checks, not reasons to weaken their accounting.

This addresses capacity deadlock when suspension is eligible; it cannot make
erroneous infinite invocations, logical dependency cycles, or unavailable
infrastructure finish. The original incident linkage remains unproven.
