# RPC suspension and admission

Pending durable RPC operations register a suspendable wait when the operation starts, including
the interval before an async result is consumed. After `rpc_suspend_after` (default 30s), the await
checks the same `safe_to_suspend` voluntary-sleep predicate used by other suspendable waits. If an
outgoing HTTP transmission, another live host call, or an open durable scope defers yielding,
eligibility is checked again after `wait_suspend_check_interval` (default 10s).

Once eligible, the executor first persists a scheduler wakeup `rpc_resume_after` later (default
5s). Mixed waits choose the earliest wakeup. The worker then suspends and later follows ordinary
Store reconstruction, reusing the same logical RPC key. This is proactive resource scheduling,
not a recoverability boundary: it never gates explicit interruption or arbitrary Store loss. It
adds no recovery mechanism, RPC deadline, Wasmtime change, phase query, overcommit, or immediate
restart.

RPC preparation acquires no target execution capacity. An execution-needing target invocation
durably enqueues its acceptance prefix before reporting acceptance; caller cancellation cannot
tear that prefix apart. Settled read-only cache hits and coalesced followers intentionally persist
no separate invocation, result, or alias. Only the executing miss owner uses normal durable
admission.
