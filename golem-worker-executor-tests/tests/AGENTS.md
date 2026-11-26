# Worker executor tests

Test-running mechanics are in the `testing` skill; test patterns, tooling and exemplar tests for
durable execution are in `understanding-durable-execution` (`reference/testing-patterns.md`).
Tests here must never spawn external processes (see the repository `AGENTS.md`).

## Human-readable tests

- Write each test as setup, action, then checks. Split independent scenarios into separate tests. Give phases that must share state across a restart their own named functions.
- Use `golem-worker-executor-test-utils` and `golem-test-framework::dsl::TestDsl` as the main test interface. Extract repeated setup and protocol handling there; each new DSL helper must serve at least two tests.
- Return fixtures as structs with named fields, not positional tuples. Keep server handles with the fixture so their lifetime is visible.
- Build schema types and values with `IntoSchema`-derived structs, `data_value!`, or `IntoTypedSchemaValue`, rather than hand-built schema/value ADTs.
- Group checks by the guarantee they prove, using the testing-plan wording when available. Explain non-obvious assertions with messages. Match error codes or typed error variants, not substrings of formatted errors.
- Put imports at module scope and use short imported paths. Name timeouts and oplog positions instead of repeating literals.
- Use a shared oplog-shape assertion for the invariants below. Make any permitted incomplete call or retained terminal explicit at the call site; do not weaken the common assertion for one scenario.
- A readability refactor must preserve every scenario, crash window, effect counter and assertion. Run the same affected tests before and after it.

## Prefer production behavior over test-only runtime plumbing

Exercise lifecycle and concurrency behavior through production APIs, existing dependency
boundaries, controllable I/O or resource contention, or colocated unit tests of the actual
production primitive. Avoid adding feature-gated runtime fields or branches, test-only setters or
private-state mutation, and always-on no-op hooks solely to pause a test. Existing hooks are not a
precedent for adding more.

This is a preference, not an absolute ban. An unavoidable hook needs a concrete explanation of the
otherwise untestable guarantee and must have the smallest practical scope. Do not replace causal
synchronization with sleeps, retries, weaker assertions, reduced concurrency, or a copied state
machine. Timeouts bound failures; they do not establish ordering. Bound a wait with
`within(timeout, what, future)` from `golem_test_framework::dsl`, so that a timeout names what
the test waited for, and use `GATE_TIMEOUT`, `SETTLE_TIMEOUT` or a named constant of the file.
Wait for a stdout line with `WorkerLogEventStream::wait_for_stdout`. Check a rejected or failed
invocation by its worker error with `assert_worker_error!`, not by the text of the error.

Put a test double at the boundary where the executor leaves its process, and keep the
production code on the executor's side. To change some blob-storage operations, implement
`BlobStorageDecorator` and pass `wrap_blob_storage()`. For calls the executor makes to the worker
service, such as card transfers, start an `EmulatedWorkerService` and pass it with
`TestExecutorOverrides::with_worker_service`. For permission cards that the registry keeps,
start an `EmulatedRegistryService`, add the initial cards, and pass it with `with_registry`.
Do not write another wrapper of `BlobStorageBackend`, `WorkerProxy` or `CardService` for these
cases.

## Rules for durability, replay and RPC tests

- **Count effects, not echoes.** Equal deterministic return values do not prove deduplication.
  Assert a provider-side counter (target agent counter, wrapped service call count, external
  stub) and, separately, that the next fresh invocation continues from that count.
- **Assert identity and execution count separately.** Capture the idempotency key each attempt
  carries (`TestExecutorOverrides::wrap_rpc`), assert equality, and assert one
  `AgentInvocationStarted` for it in the target oplog.
- **Crash at named windows.** Gate the crash before `Start`, after the effect but before `End`,
  after `End` before delivery, after `AgentInvocationFinished`, during the drain before
  `AgentInvocationSettled`, during snapshot load, and during an update, using `simulated_crash`,
  wrapped services, or a guest method that effects-then-traps.
- **Restart for real.** `executor.restart(Teardown::Drop | Shutdown | Release)` (in-process
  executor teardown and recreation over the same storage with the same start function — a
  genuine reconstruction, not OS process death; `restart_with` changes the overrides on purpose),
  or `simulated_crash`; a cursor reset or `resume` on a resident worker is not a restart.
  Then check both guest-visible state and oplog shape (`get_oplog`, `search_oplog`): one
  `Finished` per key; every `Start` in successfully settled history has a terminal, except a
  `LeaveIncompleteOnDrop` call that was in flight when its entity body was cancelled; any other
  incomplete `Start` is expected only where the crash was injected; no positional `Start`/`End`
  of an invocation after its `AgentInvocationSettled`, or after its terminal
  (`AgentInvocationFinished` or `AgentInvocationFailed`) when it writes no `Settled`
  (methods that use streams, through stream inputs or a stream result, snapshot save and load
  including a manual update's, oplog-processor batches and external-tool invocations). `Start`/`End` entries between an invocation's terminal
  and its `Settled` are its background work and are expected. The exception is the terminal of a
  retained call or deferred destructor that a later invocation drives, or a task parked at a safe
  park point (see `src/worker/AGENTS.md`). Check these rules with `OplogShape` from
  `golem-worker-executor-test-utils` instead of a local copy, and name each exception with its
  builder method (`crashed_during`, `left_incomplete_on_drop`, `settled_by_later_invocation`,
  `in_flight`); select the entries of one call or invocation with `OplogView`.
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
