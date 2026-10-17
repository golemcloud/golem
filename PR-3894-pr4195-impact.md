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

The subsequently fetched main also includes #4220, #4223 and #4224. Its tip is
`e0612d6332661233ef1ecd5b6d38ef69ab74a4f9`. Local integration follows the observer removal's verified
small-batch checkpoint. The only textual merge conflict registered
two independent tool-test modules; both `dropped_waits` and `durable_entity_blob` are retained.

#4220 captures accessor identity in the initiating guest call and moves its Start/claim into an
owned Store task. The task holds tail-work activity and races the ordinary worker interrupt source.
This must compose with the branch's InterruptSources, not bypass it. Async request builders now
receive the owned task's accessor; plain requests move as values. Dropped `wait-for` host-internal
clock reads are drained only through their declared owner's replay-inert subtree. Preserve that
call-local contract; do not interpret it as permission to skip arbitrary missing Starts or history.
Keep the upstream first-await placement for `wait-until` and its real cancellation checkpoints.

#4223 ignores an existing Go snapshot-load assertion pending GOL-611. Preserve the upstream status,
do not count it as passing and do not broaden replay to hide its runtime-state divergence. #4224
strengthens a read-only follower check from entry count to ordered entry types; its existing
GrowMemory exclusion is not permission to weaken this branch's complete committed-prefix checks.

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

Combined-source verification uses this process:

1. Checkpoint the verified observer removal and runbook before merging. This is done locally.
2. Integrate main locally at a separate checkpoint. Do not push, amend, resolve review threads or
   change tracker status as part of this investigation.
3. Rebuild the executor and affected Rust fixtures separately from test execution. The host-API
   fixture is essential because its custom-durability methods and SDK imports changed. Old binaries
   and old Wasms do not exercise the new guest wrapper, even if host-only checks pass.
4. Record the new source and fixture identities. Keep all earlier receipts tied to their old trees.
5. Use the local runbook's small explicit batches, thermal guard and CI-aligned 20-minute maximum.
   The guard is validated and execution has resumed. No I/O guard or broad integration rerun is part
   of this plan.

## Focused verification

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

The durable-primary composition cases are implemented in
`tests/api/interruption/lifecycle/custom_durability.rs`:

- `concurrent_custom_promises_auto_suspend_and_reconstruct_original_scopes` uses actual concurrent
  SDK promise waits and default automatic-suspension policy. It observes Suspended and physical
  release before completing the promises through the public API. It checks original sibling Starts,
  delivery markers, the later root call's ordinal, one invocation result and the full stored prefix.
- `quota_stop_retains_pending_custom_finish_and_replays_its_markerless_end` and its second-sibling
  counterpart disable only automatic suspension for that scenario. They hold the selected custom End
  receipt at the existing oplog dependency and apply a real monthly memory-policy update. They
  require accepted quota interruption while the permit and finish remain held, then check physical
  cleanup, a markerless End, an incomplete sibling, positive settlement without another closed-window
  charge and ordinary reconstruction. Completed-body observations must not repeat. The guest fixture
  adds only public promise workloads, not a Worker hook or forced-expiry path.

These cases do not cover custom ephemeral failure, a custom provider mutation or every meter. The
upstream HTTP sibling/effect tests and the existing backend-effect and resource-mode cases remain
separate proof obligations. Initial compile and native test mistakes are retained in the local
receipts. The corrections use production Context for production-state queries and the canonical
payload descriptor for the later root call; no runtime change or weaker assertion was needed.
The host-API/HTTP/tool fixtures rebuilt and validated on the new SDK. Upstream custom scope/drop/
recovery cases and all five dropped-wait restart cases pass in their small guarded batches. The
changed HTTP replay-admission cancellation and recorded clock deadline/cancellation cases pass too.

The quota cases retain the complete committed prefix through the stopped End, the exact selected
sibling's terminal and a newer resident generation. A final-cohort timeout prompted test-only
phase/history diagnostics. The diagnostic pass did not clear the earlier failure. A reverse-order
regression then reproduced a test-targeting defect before quota application: the selected sibling
already had End and Delivered, while the function-name gate remained attached to the other Start.
Concurrent Start observation order need not match logical initiation order.

The test now waits for both real body Starts and arms the same dependency fixture against the exact
selected Start before completing its promise. The test utility reuses its existing End-gate map and
receipt path; it adds no Worker state, new runtime branch or scheduling checkpoint. Both completion
orders remain concurrent, with identical deadlines and assertions. Temporary phase/history probes
are removed. Both orders now pass in the same red-capable guarded cohort, as do the existing
single-call gate consumer and monthly-acceptance regression. The original
unattributed timeout and the explicit reverse-order RED remain recorded; neither is waived as a
flake or replaced by the intervening diagnostic pass.

Expanded library checks, strict executor/test-utility lint, production compilation and the updated
walkthrough's offline browser check pass. The final terminal-ownership library check includes failed
End persistence during teardown. This is selected combined-source verification, not full-suite,
native managed-XFS or universal custom-operation coverage. No suspension-coordinator rewrite or new
lifecycle gate was needed.

These are specific composition checks, not permission to create Worker test setters, fake runtime
observations, replay tolerance, ordering sleeps, retries or another lifecycle coordinator. Do not
claim additional assertions pass until the combined code and new fixtures actually exercise them.

The fetched-main verification queue must also include small exact-name batches for #4220's
`tool_streaming::dropped_waits` cases and its changed HTTP/clock cancellation cases. Rebuild their
affected host-API, HTTP and tool-provider fixtures on the combined SDK/WIT before running them.
Automatic suspension must continue to treat an outstanding owned Start/claim as active work.
Quota-forced interruption is different: include a case where the guest has dropped a wait and
advanced to its real checkpoint while its owned Start is still held, then stop through the ordinary
monthly path. Verify the exact committed history and reconstruction after releasing the real append
dependency. The worker-interrupt race in `hand_over_owned_start` is not, by itself, proof that this
guest-drop/whole-Store-stop conjunction has been covered. This is a verification gap to exercise,
not a diagnosed defect or a reason to add another lifecycle gate.

## Primary sources

- [Merged host implementation](https://github.com/golemcloud/golem/blob/4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d/golem-worker-executor/src/durable_host/durability.rs)
- [Merged completion delivery](https://github.com/golemcloud/golem/blob/4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d/golem-worker-executor/src/durable_host/concurrent/delivery.rs)
- [Merged Rust wrapper](https://github.com/golemcloud/golem/blob/4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d/sdks/rust/golem-rust/src/durability.rs)
- [Merged WIT contract](https://github.com/golemcloud/golem/blob/4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d/wit/deps/golem-durability/golem-durability.wit)
- [Merged native tests](https://github.com/golemcloud/golem/blob/4cab5c07c0f7588f0a5d8f64b0485533fb6fb85d/golem-worker-executor/tests/durability.rs)
- Local `worker/suspension.rs`, `durable_host/durability.rs`, `durable_host/mod.rs` and
  `durable_host/concurrent/delivery.rs` for the surviving integration owners.
