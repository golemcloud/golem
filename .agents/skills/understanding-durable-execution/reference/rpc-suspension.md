# RPC suspension and admission

Owned async RPC tasks register an activity when dispatched, including the interval before their
result is consumed (`durable_host/wasm_rpc/mod.rs::spawn_rpc_task_with_retry` and accessor result
waits). After `rpc_suspend_after` (default 30s), that activity may permit automatic suspension,
but only through the shared `OwnerSuspension` authority (`worker/suspension.rs`). Borrowed
synchronous RPC and preparation remain unclassified vetoes: raw sync RPC does not auto-suspend.
Every admitted primary/entity/native participant must be accounted for. A runtime blocked
witness alone is insufficient; an active outer poll, unknown non-root runtime task, active
journaling/publication or other unclassified work denies eligibility. Passive stream source
waits are tied to the current blocked owning Store, not treated as general idle work.
The owner policy in `RuntimeStore::drive` rechecks after `wait_suspend_check_interval`
(default 10s), following its initial `wait_suspend_grace`.

Once eligible, the executor first persists a scheduler wakeup `rpc_resume_after` later (default
5s). Mixed waits choose the earliest wakeup. After persistence, the same activity revision and
eligibility are revalidated; readiness or new work invalidates stale evidence. Owned P2 timer-only
`poll`/`block` dispatch (`durable_host/io/poll.rs`) releases the Store borrow and preserves timer
indices, including duplicates; other pollables and `ready` are not generally adapted.
The worker then suspends and later follows ordinary
Store reconstruction, reusing the same logical RPC key. This is proactive resource scheduling,
not a recoverability boundary: it never gates explicit interruption or arbitrary Store loss. It
adds no recovery mechanism, RPC deadline, cleanup barrier, lifecycle protocol, overcommit, or
immediate restart. Runtime observations supply facts, not suspension authority. Existing
timestamped Suspend, interruption/retirement precedence and discard/replay remain unchanged.
Unclassified borrowed waits (`durable_host/suspendable_wait.rs::wait_for_ready`) only observe
readiness and interruption.

RPC preparation acquires no target execution capacity. An execution-needing target invocation
durably enqueues its acceptance prefix before reporting acceptance; caller cancellation cannot
tear that prefix apart. Settled read-only cache hits and coalesced followers intentionally persist
no separate invocation, result, or alias. Only the executing miss owner uses normal durable
admission.
