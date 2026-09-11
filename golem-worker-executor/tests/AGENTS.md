# Worker executor tests

Test-running mechanics are in the `testing` skill; test patterns, tooling and exemplar tests for
durable execution are in `understanding-durable-execution` (`reference/testing-patterns.md`).
Tests here must never spawn external processes (see the repository `AGENTS.md`).

## Rules for durability, replay and RPC tests

- **Count effects, not echoes.** Equal deterministic return values do not prove deduplication.
  Assert a provider-side counter (target agent counter, wrapped service call count, external
  stub) and, separately, that the next fresh invocation continues from that count.
- **Assert identity and execution count separately.** Capture the idempotency key each attempt
  carries (`TestExecutorOverrides::wrap_rpc`), assert equality, and assert one
  `AgentInvocationStarted` for it in the target oplog.
- **Crash at named windows.** Gate the crash before `Start`, after the effect but before `End`,
  after `End` before delivery, after `AgentInvocationFinished`, during snapshot load, and during
  an update, using `simulated_crash`, wrapped services, or a guest method that effects-then-traps.
- **Restart for real.** Drop the executor and `start_with_overrides` again (in-process executor
  teardown and recreation over the same storage — a genuine reconstruction, not OS process
  death), or `simulated_crash`; a cursor reset or `resume` on a resident worker is not a restart.
  Then check both guest-visible state and oplog shape (`get_oplog`, `search_oplog`): one
  `Finished` per key; every `Start` in successfully settled history has a terminal (an
  incomplete `Start` is expected only where the crash was injected); no positional `Start`/`End`
  after the last `Finished`.
- **Prove no repeated effect on completed replay.** Stop or count the dependency before the
  restart; replay of a completed call must not touch it.
- **Use asymmetric concurrent orderings.** Completion order must differ from initiation order in
  at least one permutation.
- **Streams and tool bodies: crash at checkpoints, assert by offset and by `Start`.** Crash the
  producer after a committed `StreamItems` and the consumer after a journaled item; assert each
  item is observed once and one terminal is recorded. Drive tool bodies to named checkpoints with
  the in-process crash-checkpoint server in `tool_streaming.rs` (never an external process) and
  assert one entity terminal per `Start` and no repeated external effect on completed replay
  (the body's guest export does run again; count what it calls, not whether it ran).
- **Replay errors are findings.** `unexpected_oplog_entry`, "no matching Start", extra entries or
  a changed pending-completion set mean a determinism bug. Never retry, loosen, or add tolerance
  to make such a test green.
