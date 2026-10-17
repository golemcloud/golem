# PR #4195 impact on monthly limits and suspension

#4195 affects the integration work. It does not directly change the suspension coordinator or
monthly policy, but it changes the custom-operation execution and completion paths they must work
with. Preserve the existing interruption and reconstruction owners; no new suspension protocol is
indicated by this review.

This is a static assessment made before integration. No build, test, benchmark, fetch or merge was
performed for that assessment. At review time the branch had the old Rust custom-durability
implementation and an uncommitted monthly-observer removal. Subsequent guarded verification of that
observer change is recorded separately; it does not validate composition with #4195.

## Upstream change

[PR #4195](https://github.com/golemcloud/golem/pull/4195) merged on 2026-10-17. Its base is the same
main revision already integrated here, `7f3615d51d6f7369c3f9675db367306aca65c72c`. The merge is
`4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d`.

The fix is for concurrent **custom durable operations**, not a replacement for ordinary built-in
durability or Worker suspension:

- Rust async operations begin on their first poll. Constructing an unpolled future does not begin
  its custom invocation or reserve its identity.
- The SDK installs custom attribution around each body/finish poll and destruction, then restores
  the actual caller. A pending sibling does not become the accidental parent of unrelated work.
- New synchronous context exchange and resident snapshot resources implement that switch. They
  neither journal entries nor preserve scopes across Store loss.
- Async finish borrows its live resource, persists End through an owned append/commit task, and
  releases the result through ordinary CompletionDelivery.
- Completed replay skips the body but waits for the recorded delivery marker or natural replay tail.
  An incomplete sibling repairs under its original Start. Later root-call ordinals stay unchanged.
- Finish checks active children and decodes the response before consuming its resource. Pending
  persistence failures retain the custom call's own Start as their trap context.
- Rust poll/drop propagation is fixed. The other SDKs receive bindings, not equivalent concurrent
  continuation propagation. Do not claim a cross-SDK concurrency fix.

## Suspension impact

| Path | Assessment and required check |
|---|---|
| Automatic suspension on eligible timers/promises/RPC waits | OwnerSuspension and RuntimeStore are not changed by #4195. They classify actual runtime activities, not custom-scope stack depth. New async admission/finish changes the import activity graph, so verify that real eligible body waits still suspend and active work still vetoes automatic suspension. |
| Monthly quota-forced Suspend | The monthly monitor still queues an ordinary Worker interruption. It must not wait for automatic-suspension eligibility or for a custom body to become a preferred unload point. Existing owners must unwind the guest and retain entered effects and accepted persistence. |
| Async finish already persisting End | CompletionDelivery now retains the accepted terminal task and live-call permit. A stop must not lose that join, refund before actual window closure, report successful cleanup while work remains, or append a second terminal. Persistence failure must remain distinct from quota exhaustion. |
| Guest cancellation versus Store teardown | Intentional guest cancellation can write CompletionDiscarded only after successful End. Whole-Store loss must leave an undelivered End markerless for reconstruction. Check the existing teardown probe and drop-event drainage under the actual quota stop. |
| Ordinary reconstruction after Suspend/Restart | Resident context snapshots cannot be carried into a new Store. Guest replay reconstructs the scopes. Completed custom bodies stay skipped; incomplete bodies repair at their own Start, without changing sibling or later-root identities. |
| Snapshot and completion boundaries | The new live-call permit participates in existing in-flight-host-call accounting. Preserve ordinary snapshot/checkpoint and invocation-finished boundaries while finish persistence or delivery markers are outstanding. |

The synchronous exchange import matters here: it switches context on the actual guest task without
introducing an async wait. It is not a new suspendable operation or a readiness token. Do not add it
to suspension classification as an eligible passive wait merely because its snapshot is resident.

The new replay marker waits are also not a reason to classify all custom imports as neutral or
eligible. Unknown active imports already veto automatic suspension, except for the certified root.
Verify any topology issue against actual runtime observations before changing that rule.

## What upstream coverage does and does not establish

The added `custom_durability_recovers_with_one_completed_and_one_suspended_sibling` test uses a
withheld HTTP response followed by `simulated_crash`. Here "suspended sibling" means a pending Rust
future, not a Worker automatically entering Suspend or being stopped by a monthly cap.

It proves original sibling identities, one completed sibling not repeating its effect, repair of the
incomplete sibling, preserved later-root identity and no Jump-based rollback. Its incomplete sibling
legitimately performs its external effect again. Do not turn this into an exactly-once claim for an
external peer without an idempotency contract.

The new finish-cancellation tests cover accepted append ownership and call-specific failure
attribution. They are valuable prerequisites, but they do not by themselves prove monthly monitor,
accounting, permit release or automatic-suspension composition. That remains additional coverage.

## Integration boundary

The local delta in `durable_host/durability.rs` against the PR's base is the composed InterruptSources
implementation. Keep its execution-status subscription, deadline latches, owner failure and entity
cancellation behavior when integrating upstream. #4195's scope exchange and completion ownership
must remain intact alongside it. Neither change justifies restoring publication/seal/result-writer
coordination removed from this branch.

Bring the merged main change as a coherent SDK/WIT/host change rather than copy only the host methods.
The host API, bindgen synchrony, Rust wrapper, completion delivery, fixture methods and documentation
all participate in the new contract. Keep the exchange import synchronous.

Review documentation overlaps in the durable-execution skill and walkthrough. The small Linux cfg
fixes in filesystem and test utilities are separate from suspension behavior and should not be
reinterpreted as a quota design change.

Before execution on the combined source:

1. Preserve the current uncommitted observer-removal edits and runbook. Do not discard them to merge.
2. Integrate main locally at a separate checkpoint. Do not push, amend, resolve review threads or
   change tracker status as part of this investigation.
3. Rebuild the executor and affected Rust fixtures separately from test execution. The host-API
   fixture is essential because its custom-durability methods and SDK imports changed. Old binaries
   and old Wasms do not exercise the new guest wrapper, even if host-only checks pass.
4. Record the new source and fixture identities. Keep all earlier receipts tied to their old trees.
5. Use the local runbook's small explicit batches, thermal guard and CI-aligned 20-minute maximum. No I/O guard
   or broad integration rerun is part of this plan. Execution remains paused for cooling and guard setup.

## Focused verification queue, not executed

Start with existing upstream proofs, using exact test names in small related batches:

- `durable_host::concurrent::tests::completion_delivery_terminal_failure_retains_call_attribution`
- `durable_host::concurrent::tests::completion_delivery_cancelled_terminal_wait_preserves_append_ownership`
- `durability::custom_durability_scopes_follow_poll_and_drop_execution`
- `durability::custom_durability_cancelled_finish_drops_result_in_its_scope`
- `durability::custom_durability_recovers_with_one_completed_and_one_suspended_sibling`

Then add the missing composition cases through existing runtime/provider boundaries:

1. **Automatic suspension inside concurrent Rust custom bodies.** Use real eligible timer or promise
   waits, observe Worker suspension and ordinary reconstruction, then check parent/ordinal identity,
   effect attribution and later-root identity. A pending async import alone is not suspension proof.
2. **Quota stop during a custom body and accepted async finish.** Keep real I/O and terminal
   persistence held at their dependency boundaries. Apply an ordinary monthly capacity change. Keep
   the exact durable Suspend or ephemeral resource error, committed raw prefix, accepted-task join,
   correct delivery/discard markers, settlement, physical cleanup and same-invocation reconstruction.
   Cover a persistence error separately; do not allow it to become a quota error.

These are specific composition checks, not permission to create Worker test setters, fake runtime
observations, replay tolerance, ordering sleeps, retries or another lifecycle coordinator. Do not
claim either case passes until the combined code and new fixtures actually exercise it.

## Primary sources

- [Merged host implementation](https://github.com/golemcloud/golem/blob/4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d/golem-worker-executor/src/durable_host/durability.rs)
- [Merged completion delivery](https://github.com/golemcloud/golem/blob/4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d/golem-worker-executor/src/durable_host/concurrent/delivery.rs)
- [Merged Rust wrapper](https://github.com/golemcloud/golem/blob/4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d/sdks/rust/golem-rust/src/durability.rs)
- [Merged WIT contract](https://github.com/golemcloud/golem/blob/4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d/wit/deps/golem-durability/golem-durability.wit)
- [Merged native tests](https://github.com/golemcloud/golem/blob/4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d/golem-worker-executor/tests/durability.rs)
- Local `worker/suspension.rs`, `durable_host/durability.rs`, `durable_host/mod.rs` and
  `durable_host/concurrent/delivery.rs` for the surviving integration owners.
