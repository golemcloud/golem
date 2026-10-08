# GOL-706: suspension-only delivery plan

Updated 2026-10-08. This is the authoritative execution plan for
[GOL-706](https://linear.app/golem-cloud/issue/GOL-706). It supersedes all execution
sequences in `gol-706-suspension-design.md`; that file retains historical evidence.

## Outcome and boundaries

Automatically suspend a durable owner only when all its resident execution is
provably unable to make useful progress until a sufficiently distant deadline or
a supported durable activation. Persist required wakeups, avoid lost-wake races,
and resume using the existing discard-and-replay lifecycle. Never suspend merely
because one wait or one runtime poll is pending.

Preserve supported P2/P3, promise, RPC, tool/native and stream behavior. Ephemeral,
quota/fuel, explicit interruption, deletion and shard-loss semantics stay unchanged.
One suspension authority replaces local heuristics; no permanent Unknown exclusions
or disabling suspension to make tests pass.

Approved contract adjustment: borrowed synchronous RPC waits never automatically
suspend and block suspension of the whole owner. This includes borrowed target
preparation reached by the async API, but not its subsequent owned result wait.
Ordinary SDK calls use async invocation plus future.get; their automatic suspension
and single-slot progress remain required. Raw synchronous callers have no single-slot
progress guarantee. Explicit interruption and crash recovery remain unchanged.
This explicit support boundary replaces the positive borrowed-observer requirement;
the M4d experiment is retired as unnecessary, not marked successful.

Not in scope: generic cleanup acknowledgments, new Store teardown, accepted-tool
ownership extraction, shared atomic-state redesign, metadata/replay-finalization
refactoring, a second lifecycle state machine, or scheduler/database redesign.
An additional change needs a concrete suspension-specific semantic failure and
coordination with the owning work. A failed prototype bookkeeping assertion is not
such evidence. Necessary failures cannot be silently deferred or waived.

## Progress board

Status values: NOT STARTED, ACTIVE, BLOCKED, REVIEW, DONE. A milestone is DONE only
when its exit evidence and required reviews are recorded. Historical prototype
passes do not confer completion on the reduced implementation.

| ID | Milestone | Status | Exit record |
|---|---|---|---|
| M0 | Remove out-of-scope prototype, preserve evidence | DONE | Baseline restored; archive verified; Oracle APPROVED/ON TRACK; bug-finder clean |
| M1 | Pin integration baseline and executable acceptance suite | DONE | Six reproduced failing cases (one intermittent), eight controls pass; Oracle ON TRACK; bug-finder clean after baseline-evidence correction |
| M2 | Reliable runtime-idle evidence | DONE | Policy-free observer and queued-work accounting; 3 unit/3 integration tests pass; Oracle APPROVED/ON TRACK; bug-finder post-fix clean |
| M3 | First end-to-end suspension slice | DONE | 9 unit tests and 5 final integration races/controls pass; Oracle APPROVED/ON TRACK; bug-finder run 2 clean |
| M4 | Complete supported participant and wait coverage | DONE | M4a–c and M4e–l verified; A1–A7 evidence reconciled; Oracle APPROVED/ON TRACK; checkpoint bug-finder gates clean |
| M5 | Final regression, scope and reproduction gate | DONE | Broad correctness evidence and unchanged published-pin isolated controls pass; full group4 timeout retained as residual risk; Oracle explicitly approves closure, final bug-finder clean |
| M6 | Deliver reviewed change | DONE | Published locked pin/build, aligned docs, reviewed implementation pushed to existing drafts; exact results and residual risk recorded; merge/release remain separate |

Current position: M0–M6 and the Wasmtime review follow-up are complete for authorized
delivery under the approved RPC contract and disclosed verification caveats.
Final fork/main integration is ACTIVE; the earlier verified runtime was
[d853214](https://github.com/golemcloud/wasmtime/commit/d853214fa72f038344a266f2a7ee66a387144257)
in [the companion PR](https://github.com/golemcloud/wasmtime/pull/9), now merged by the user.
The root manifest now uses the normal fork branch, locked to its merge revision;
merged-source validation is pending, not covered by the earlier results.
Golem follow-up [aac940237](https://github.com/golemcloud/golem/commit/aac940237)
is pushed to [PR #4062](https://github.com/golemcloud/golem/pull/4062).
Merging main into this branch and pushing the update is authorized; merging PR4062
itself and release remain separate. The original Golem baseline is
[`accae0e4`](https://github.com/golemcloud/golem/commit/accae0e435b5097cd1d7940a5c1f568bde8a055a).
Prototype recovery files are in `tmp/gol706-reduction-20261005/`. Ignored binaries
and WASMs may be stale; they are not baseline evidence.

## Wasmtime review follow-up — VERIFIED AND PUSHED

The user approved incorporating and verifying published fork revision
[d853214fa72f038344a266f2a7ee66a387144257](https://github.com/golemcloud/wasmtime/commit/d853214fa72f038344a266f2a7ee66a387144257)
from the independent PR9 review. This supersedes the earlier dependency pin;
historical results remain labelled with their original source. There is no lifecycle
or suspension-policy change.

The production installation now propagates the fallible observer-installation error
through the existing anyhow → WorkerExecutorError conversion; four unit-test
installation sites assert success. Removing the runtime's `Unknown` activity kind
needs no Golem classification change: every new runtime activity remains unknown
until Golem explicitly classifies it. All 35 lockfile replacements are Git source
changes, with no version/dependency drift or local override.

Lock audit: the private owner mutex guards IDs and accounting metadata, never a
Store, future or waker. RuntimeStore::drive releases it before polling; commit
releases it before Notify; ExternalReceiveWake and RpcWake release accounting locks
before forwarding wakes. RPC lock ordering is poll → owner, never the reverse.
Store/driver/host destruction occurs outside these guards. Oracle agrees there is
no identified owner-lock reentrancy cycle. The fork review clarified that short,
independent bookkeeping critical sections and brief cross-participant contention
are permitted; waiting for runtime progress, runtime re-entry and holding a lock
across runtime poll/drop/wake are prohibited. Every notification remains synchronous;
no invalidations are dropped or deferred. Its documentation-only clarification is
not yet published; verification targets d853214 exactly.

Added verification: the existing prepared-suspension test wakes from another OS
thread; its parent waker asserts the owner lock is available and eligibility is
already invalidated before wake forwarding. A new minimal component parks in a
borrowed host fiber, then drops the driver and Store with the real RuntimeStore
observer; the host drop guard checks lock availability and final activity cleanup.
All follow-up gates pass against d853214 without local overrides:

- Locked library/integration build: success, 36m08s (`build.log`).
- Full executor units: 2659 passed, 8 existing ignored, 22.596s (`units.log`).
- Both cross-thread wake and parked-fiber teardown regressions: pass, including
  10 additional paired runs (`races-repeated.log`).
- Targeted suspension integration: 25 passed, no ignored/failed, 434.634s
  (`suspension-integration.log`). Covers P2/P3 timers and HTTP overlap, promises,
  synchronous RPC veto, owned/unobserved async RPC, tool entities, durable stream
  unload/reconstruction, replay and explicit interruption.
- Seven original timing-sensitive regressions pass twice more: 7/7 in 101.752s
  and 102.401s, giving three runs including the targeted suite
  (`regressions-2.log`, `regressions-3.log`).
- `cargo clippy --locked -p golem-worker-executor --lib --test integration
  --no-deps -- -D warnings`: success, 11m09s (`clippy.log`).
- `cargo test --locked -p golem-common --lib --
  precompiled_components_are_compatible_across_engines --report-time`: 1 passed
  (`engine-compatibility.log`).
- Scoped formatting and whitespace checks pass. Cargo commands use
  `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
  CARGO_BUILD_JOBS=4`; logs are under `tmp/gol706-fork-followup/`.

Oracle APPROVED/ON TRACK after resolving the contract wording; all its validation
conditions are now met. Bug-finder `gol706-wasmtime-followup` run 1 is clean.
Implementation and tests are committed and pushed as
[aac940237](https://github.com/golemcloud/golem/commit/aac940237).
These targeted results do not erase the earlier full-group4 timeout or the
GOL-761/GOL-769 follow-ups; the full group4 suite was not repeated for this pin.

## Final fork/main integration — ACTIVE

The user confirmed Wasmtime PR9 is merged and authorized restoring the usual fork
branch, merging latest Golem main, resolving conflicts, verifying and pushing the
PR4062 update. This does not authorize merging PR4062 itself.

| Step | Status | Evidence / remaining gate |
|---|---|---|
| Restore published runtime | DONE | `golem-wasmtime-v46.0.1-p3`, locked to [7034e424](https://github.com/golemcloud/wasmtime/commit/7034e424b2e584cbc351d380b5b2cef8cb5e4fdb); no local override |
| Integrate current main | DONE | Merge of [6c6e4c51f](https://github.com/golemcloud/golem/commit/6c6e4c51f); source/test/doc conflicts resolved, combined tool fixture lock regenerated |
| Merged-source correctness checks | ACTIVE | Locked executor/CLI builds and fixture rebuilds pass; full executor units: 3827 passed, 20 ignored; 45 integration tests running; clippy and engine compatibility pending |
| Independent review | ACTIVE | Oracle: ON TRACK, no production blocker; both recommended composition tests pass; bug-finder run 2 clean; final Oracle pending |
| Record and deliver | ACTIVE | User authorized pushing the integration while final checks run so CI can start; no readiness claim until remaining checks finish |

Resolutions preserve main's executor-owned task shutdown and snapshot behavior while
retaining owner suspension participants/external-activity guards. The obsolete RPC
wait-registration mechanism is not restored: `RpcTask::drop` synchronously revokes
the new activity, which remains an unknown veto until actual cleanup. Added tests
exercise dropping the actual RPC wrapper before cleanup and releasing retained-Store
suspension accounting when executor shutdown prevents its callbacks from running.
Main's renamed status API replaces the old attached-status accessor in RPC retry.
Stream regression helpers use main's `golem_schema::proto` types instead of the
removed gRPC re-exports; their content assertions are unchanged.
Both sides' native tool tests, clock-race and environment/matrix fixtures are retained.
Documentation is merged and the affected rendered sections inspected.

The first bug-finder attempt and an executor build ran out of disk before testing;
neither counts as a pass. Obsolete generated build caches were removed. A later
process/session interruption stopped queued checks without final results; the
remaining build/lint checks now use supervised services and explicit exit files.
The completed unit run (`units.log`) passes both new composition tests, cross-thread
wake and parked-fiber drop cases. Bug-finder run 2 is clean. Fixture builds use the
current CLI and local Rust SDK; no fixture migrations were generated, and the caller
lock adds only the required clock client to main's lock. Root Cargo regeneration
preserves the package/version set while resolving the fork source and nine existing
transitive dependency edges; there are no hand-edited lock entries.

New evidence is kept separately under `tmp/gol706-final-merge/`. Earlier passes above
are pre-merge evidence only. GOL-761/GOL-769 remain approved follow-ups; new correctness
failures in this integration are not waived by those follow-ups.

### Typed-input checkpoint follow-up

Latest main was merged again in [d4eb65454](https://github.com/golemcloud/golem/commit/d4eb65454),
and PR fixes were pulled through [33d790541](https://github.com/golemcloud/golem/commit/33d790541).
The earlier 45-test integration run, clippy and engine compatibility checks completed
successfully, but they do not certify this later source snapshot.

The typed-input middleware test's provider checkpoint timeout reproduced in three
isolated runs on the latest PR. The decorated provider entered `get_promise_result`,
but its Start remained buffered while the test read only persisted oplog entries.
The direct case happened to suspend and flush; suspension is not a prerequisite for
this restart/content test. Its checkpoint helper now explicitly commits before each
read, using the existing test utility. The timeout, wait-entry predicate, restart,
exact content checks and duplicate-announcement checks are unchanged.

Validation: the previously failing test passes three times with the fix; logs are in
`tmp/gol706-typed-input-repro/fixed-{1,2,3}.log`. Oracle approved the narrow change
and judged it ON TRACK; bug-finder `gol706-checkpoint-persistence` run 1 found no bugs.
No production change or lifecycle refactor was needed. Full PR CI and the remaining
final integration sign-off are still separate gates.

## M1 — establish what must change and where it integrates

1. Recheck GOL-708 [PR4034](https://github.com/golemcloud/golem/pull/4034) and GOL-710
   [PR4038](https://github.com/golemcloud/golem/pull/4038). Record exact Golem and
   Wasmtime revisions and integration order. If dependencies remain unmerged, use
   an explicitly identified local integration baseline or mark the milestone blocked;
   do not silently merge, mix revisions or work around their ownership contracts.
2. Restore only behavioral regression tests/fixtures from the archive, removing
   dependencies on discarded prototype APIs. Build selected fixtures from their
   current sources using the repository workflow. Record toolchain/build commands.
3. Run the baseline matrix below, including the reported agent/tool polling-loop
   schedules. Preserve exact failures and logs; a timeout remains unresolved, not a
   passing negative control. If another PR fixed a symptom, record that provenance.
4. Specify the suspension decision boundary: which existing activity creation and
   completion sites are observed, supported wait metadata, earliest deadline,
   scheduler persistence/revalidation, and the existing lifecycle request hook.
   This is an eligibility contract, not a new recovery/cleanup protocol.

Exit: an executable list of tests and outcomes, a named base, and a bounded owner/
runtime adapter map. Any necessary unknown mechanism has one named experiment with
pass/fail/inconclusive criteria. Fail/inconclusive stops for a scope decision; it
does not create an indefinitely expanding prerequisite milestone.

## M2 — make runtime evidence trustworthy

Salvage only the necessary Wasmtime observations/activity identities and queued-work
wake correction. Cover admission before first poll, selected-but-not-executed work,
imports, AccessorTasks, roots and supported stream-transfer directions. Invalidate
evidence on wake, poll, activity change and generation teardown. Unknown resident
work blocks eligibility until classified; observation callbacks must not execute
policy or enter another generation.

Exit: executed counterexamples for fairness/yield Pending, queued guest delivery,
final-root work without a wake, stale blocked publication, activity reuse/drop and
stream work with no host import. Confirm engine configuration compatibility with
`precompiled_components_are_compatible_across_engines`. Runtime changes remain
policy-free. No cleanup-only scheduler or generic stop fence is presumed necessary.

## M3 — deliver one real vertical slice, not another policy-only prerequisite

1. Add a focused owner-wide eligibility component using existing owner/runtime
   lifetimes. Classify P3 timers and account for other activity as blockers.
2. Persist the earliest required wakeup; revalidate the exact decision after
   persistence. Readiness, new work and shorter deadlines invalidate stale decisions.
3. Request suspension through the existing lifecycle. Preserve its generation,
   cancellation, retirement and replay contracts. A demonstrated handoff lost-wake
   failure permits only a coordinated narrow correction to that lifecycle boundary.
4. Execute short/long timer races and an all-long set through the real executor:
   no premature suspension, actual unload when eligible, wake at the earliest
   deadline, replay, exact result, and a successful follow-up invocation.

Exit: production-path evidence for both refusing and performing suspension, including
readiness during persistence and wake during final handoff. No synthetic idle proof,
fake persistence receipt or new requirement to finish every disposable future.

## M4 — finish supported coverage and remove competing decisions

Complete rows A1–A7 below using existing operation ownership. Sub-checkpoints may
report progress but do not add milestones or reduce this exit gate. Every row has
an executed production acceptance test; isolated policy tests are supplementary.

| Row | Required behavior and decisive checks |
|---|---|
| A1 P3 clocks | `[2s,60s]`, `[2s,60s,600s]` do not suspend prematurely; all-long sets unload and resume at earliest deadline; cancellation/readiness invalidates obsolete plans. |
| A2 Promises | Short timer plus pending promise stays runnable; promise-only wait unloads; completion before/during/after suspend and executor restart eventually returns exactly once. |
| A3 P2 | Actual SDK long timer unloads and resumes; mixed independently scheduled P3 HTTP/P2 waits progress with one effect and follow-up; retain poll duplicates, resource lifetime, real file readiness and replay. Test relevant block/ready/read/write paths, not poll(list) alone. |
| A4 RPC | Async waits: grace then durable recheck; HTTP blocker finishing permits later suspension; one-slot progress, stable retry key, exactly-once execution/recovery. Borrowed RPC/preparation: no auto suspension and owner-wide veto; delayed raw sync call completes with sufficient capacity; interruption/replay preserved. |
| A5 Owner participants | Primary, tool/middleware and native/MCP activity count from admission through their existing completion boundaries; runnable sibling forbids suspension; eligible tool waits suspend the owner; recorded replay and follow-ups remain correct. |
| A6 Streams | Source waits versus active journaling/delivery distinguished; non-flat values/nested handles and supported transfer directions preserve exact values, offsets and terminal behavior across suspension/replay. Post-result producer activity remains visible. No invented EOF/Cancelled or extra effect. |
| A7 Lifecycle races | Wake before/during persistence, before/after request, during unload and after reconstruction is not lost; newer retirement/interrupt wins; stale callbacks cannot affect replacement; explicit interruption and quota/fuel behavior preserved. |

For P2, try only the narrow owned-timer dispatcher through production bindings.
For streams, test existing discard/replay rather than imposing new acknowledgment
barriers. If either cannot preserve supported behavior, present the exact failing
contract and smallest proposed change before expanding the fork or executor scope.

Exit: A1–A7 pass, with all supported automatic-suspension paths using the same
authority. Remove old local election/scheduling branches and temporary fallbacks;
retain ordinary readiness behavior. Account creation sites, not guessed call counts.

## M5 — prove completion on the final source

**Current scope decision (2026-10-07): correctness first.** The user explicitly
removed memory attribution from GOL706 completion gates. The observed test-process
memory excess and proposed cache experiment are deferred to
[GOL-761](https://linear.app/golem-cloud/issue/GOL-761/investigate-higher-worker-executor-test-memory-after-gol-706)
after the suspension fix is merged. Historical resource pauses below are superseded;
do not run further memory-attribution probes here. Resume correctness verification.
The user subsequently approved temporarily ignoring the short-sleep scalability
latency test. Its original p95 < 6000ms and total < 10s assertions remain unchanged;
[GOL-769](https://linear.app/golem-cloud/issue/GOL-769/restore-short-sleep-scalability-latency-test-after-investigating)
in Golem 1.6 finalization records the evidence and restoration instructions (remove
only the ignore attribute). This exception does not waive replay/suspension defects
or incomplete runs caused by resource exhaustion, and does not authorize merge or
lifecycle refactoring. Do not reopen either performance investigation here.

The original baseline obligations include these archived regression names:
- `p3_short_timer_blocks_suspension_with_long_watchdog`
- `p3_short_timer_blocks_suspension_with_multiple_long_watchdogs`
- `p3_short_timer_blocks_suspension_while_promise_is_pending`
- `p3_all_long_timers_suspend_and_resume_at_earliest_deadline`
- `clock_races_complete_through_tool_entity_without_suspending_owner`

Also retain positive controls: P3 sleep, strengthened P2 long-sleep unload assertion,
promise restart, both HTTP/P2 sleep schedules, tool long-clock race, polling-loop
agent/tool entrypoints, single-slot RPC and RPC-after-HTTP progress, durable input/
output recovery, explicit interruption, and follow-up invocation. Exact test paths
and fixture mappings are frozen at M1, not invented from the old prototype API.

Run each timing-sensitive GOL706 regression three times on final source, using
deterministic gates where feasible. Exercise the originally reported replay and
scheduler/SQLite symptoms: do not infer a connection leak or claim it fixed merely
because eligibility tests pass. If an independent defect remains, supply its
reproducer and seek explicit issue/scope disposition; until then GOL706 is not
declared fully resolved. Never silently drop an acceptance obligation.

Run the affected unit suites, runtime/fork tests, shared-engine compatibility, and
all worker-executor test groups on the final integration base. Preserve failures and
compare suspected unrelated ones against that base. No green claim with unexplained
failures. Perform package-scoped format/lint/build checks. Oracle reviews the final
diff for correctness AND scope; bug-finder must have no unadjudicated fix-worthy
findings. Final acceptance is stronger than accumulated intermediate green runs.

## M6 — deliver without hiding remaining publication work

Update the executor walkthrough and durable-execution guidance for the actual narrow
change. Verify affected walkthrough rendering. Remove experimental hooks not needed
for permanent tests, local absolute dependency overrides and generated lock drift.
If a fork patch is necessary, prepare its separately reviewable commit and reproducible
dependency pin; publishing it requires approval. Re-run checks invalidated by final
documentation/dependency changes.

Prepare reviewable local commits and final evidence with the exact Golem/fork base.
Pushing, PR creation/merge and deployment require their own authorization. Report
"implementation verified locally" separately from "submitted", "merged" or "released".
No artifact under tmp may be required for a clean checkout to build or pass tests.

## Completion checklist and reporting rule

- [x] Rejected lifecycle prototype removed and recoverable (M0).
- [x] Exact dependency baseline and original defect outcomes recorded (M1).
- [x] Runtime evidence survives executed counterexamples (M2).
- [x] Real timer decision → unload → wake → replay → follow-up passes (M3).
- [x] A1–A7 all pass; one authority; approved borrowed-RPC boundary (M4).
- [x] Original suspension report resolved; final-source acceptance and broad evidence
      reviewed; Oracle and bug-finder gates closed, with group4 caveat below (M5).
- [x] Published dependency pin builds without local overrides; docs verified;
      reviewed implementation committed and pushed to existing drafts (M6).
- [x] Authorized draft publication complete; merge/release remain unauthorized.

At every implementation checkpoint, including sub-checkpoints within a milestone,
obtain both Oracle and bug-finder review. Record source/fork revisions, commands,
decisive output/logs, verdicts and remaining findings; update this plan and Linear;
report position, work left, adjustments and Oracle ON TRACK/AT RISK/DERAILED.
Do this before declaring the checkpoint complete or starting the next checkpoint.
Milestone-end review does not replace checkpoint reviews.
Pause on DERAILED, a nonconverging review, an unresolved gate or proposed scope
expansion. Do not redefine DONE to accommodate a failure. Documentation-only plan
review uses Oracle; bug-finder applies to each implementation checkpoint.

## Checkpoint ledger

| Checkpoint | Evidence | Reviews | Remaining |
|---|---|---|---|
| M0 reduction | Exact HEAD equality; archive SHA256 and patch applicability; metadata/fmt pass | Oracle APPROVED/ON TRACK; bug-finder run 1 clean | All implementation milestones M1–M6 |
| This plan | Documentation-only synthesis; no production change or new tests run | Oracle ON TRACK; requested explicit per-checkpoint reviews/reporting, incorporated above | Begin M1; plan review is not implementation approval |
| M1a regression recovery | Rebuilt host-api/tool fixtures; three premature suspends, two timeouts; eight controls pass | Oracle APPROVED/ON TRACK for sub-checkpoint; bug-finder `gol706-m1-regressions` run 1 clean | Primary-agent polling-loop baseline and M1 acceptance record required before M2 |
| M1b closure | Primary polling loop rebuilt/executed; initially passed, repeats exposed intermittent Suspend/replay timeout; inventory corrected, tests unchanged | Oracle APPROVED/ON TRACK conditional on bug review; bug-finder `gol706-m1-primary-loop` run 2 clean after adjudicating TEST_CONFLICT as intended baseline reproduction | M2–M6; six baseline regressions remain production obligations |
| M2 runtime evidence | Fork-local balanced activity identities, queued-work lifetime, blocked generations, final-root queue progress; 3 unit/3 integration tests pass | Oracle APPROVED/ON TRACK; bug-finder `gol706-m2-runtime-observation` run 2 clean on fixed snapshot | M3–M6; patched engine compatibility at integration; full stream directions/values remain A6 |
| M3 timer slice | 9 adapter/priority tests; 5 final real-executor persistence/handoff/interrupt/all-long controls pass; prior short/polling-loop cases pass; patched engine compatibility passes | Oracle APPROVED/ON TRACK, passing-test conditions met; bug-finder `gol706-m3-timer-slice` run 2 clean | M4–M6; no full owner/wait coverage claim |
| M4a narrow P2 gate | 3 fork dispatcher tests, immediate-readiness budget regression, 6 final integration tests pass (4 actual P2, 2 P3 controls) | Oracle ON TRACK, no blockers conditional on final rerun, now passed; bug-finder `gol706-m4-p2-dispatch` run 1 clean | M4 ACTIVE: remaining file/read-write A3, promises, RPC, participants, streams, broader A7 and local-election removal |
| M4b promise integration | 12 coordinator tests; 8 final combined integration controls pass, including promise-only unload/autonomous activation, restart and short-timer refusal | Oracle APPROVED conditionally/ON TRACK, test conditions met; bug-finder `gol706-m4b-promises` run 1 clean | M4 ACTIVE: deterministic promise handoff acceptance, remaining A3, A4–A7, local-election removal |
| M4c promise handoff | Real forced status flush holds committed Suspend before unload; completion activates before release; autonomous reconstruction/exact result; 3 repeated passes | Oracle APPROVED/ON TRACK; bug-finder `gol706-m4c-promise-handoff` run 1 clean | A2 checkpoint coverage met; M4 still ACTIVE for remaining A3, A4–A7 and local elections |
| M4d borrowed-RPC probe | Scratch-only counterexample: borrowed Pending with unpolled sibling produces no blocked witness under both export ABIs; no candidate extension implemented | Oracle INCONCLUSIVE/AT RISK, pause gate; bug-finder `gol706-m4d-probe-evidence` run 1 clean for bounded evidence | Retired by approved non-suspending borrowed-RPC contract; follow-up experiment superseded |
| M4e borrowed-RPC contract | Five borrowed waits retain explicit interruption but no longer elect automatic suspension; Rust fixture rebuilt; five residency/async/replay controls pass | Oracle APPROVED/ON TRACK; bug-finder `gol706-m4e-borrowed-rpc` run 2 clean | M4 ACTIVE: async owner coordinator integration, mixed borrowed-sibling veto, remaining A3–A7 and removal of local elections |
| M4f owned async RPC | Shared coordinator replaces local RPC elections; final 21 unit and 7 integration tests pass, including unobserved RPC+timer actual unload/autonomous resume | Oracle APPROVED/ON TRACK; bug-finder `gol706-m4f-owned-rpc` run 2 clean | Mixed borrowed-sibling veto, remaining participants/A3–A7 and final regression; no full A4/M4 completion claim |
| M4g tool/entity accounting | DONE: 75 coordinator/tool-operation units and 4 production integrations pass; scalar and mixed-completion batch actually unload/reconstruct with exact results and follow-ups | Oracle APPROVED/ON TRACK; bug-finder `gol706-m4g-entity-accounting` run 1 clean | Mixed borrowed/native sibling controls, remaining A3/A6/A7, final regression and delivery; not full A5/M4 closure |
| M4h mixed-owner vetoes | DONE: borrowed synchronous RPC and real Host-native execution prevent suspension beside long timers; releasing the blocker permits actual unload/reconstruction with exact effects and follow-ups | Oracle APPROVED/ON TRACK; bug-finder `gol706-m4h-mixed-owner` run 1 clean | Remaining P2/files, streams/post-result, broader races and local-election removal; M4 stays ACTIVE |

### M5 — final-source verification: ACTIVE

The six original regression selectors passed three consecutive final-source runs
(6 passed each): `tmp/gol706-m5/original-regressions-{1,2,3}.log`.
Executor library suite: 2653 passed, 0 failed, 8 ignored
(`tmp/gol706-m5/executor-units.log`). Fork integration suites: 3 dispatcher and
4 runtime-observation tests passed (`tmp/gol706-m5/fork-tests.log`).

`p3_repeated_short_timer_races_preserve_shared_scheduler_wakeups` passed in 62s:
20 fresh short-timer owners alternate two/three timers on one SQLite executor,
return exactly 2 without Suspend and accept follow-ups; two subsequent concurrent
long-timer owners actually unload and autonomously return exactly 12. Evidence:
`tmp/gol706-m5/shared-scheduler.log`. This exercises scheduler availability after
the reported workload, not proof of a generic SQLite connection leak or its repair.
Oracle approved the cumulative implementation, scope and new test; bug-finder
`gol706-m5-final` run 1 clean. M5 is AT RISK on incomplete verification, not DERAILED.
All three shared-scheduler runs pass (`shared-scheduler.log`,
`shared-scheduler-{2,3}.log`, respectively 62.378s/60.534s/60.322s).
Fork observation units also pass
(3/3, `fork-units.log`); shared-engine compatibility passes (1/1,
`engine-compatibility.log`). Scoped Rust formatting and both diff whitespace checks pass.

Broader group1 first attempt failed during dependency initialization, before tests,
because the mixed-language RPC fixture had not been copied. Rust fixture builds
passed. TS SDK/template and TS streaming fixture builds passed. The mixed RPC
fixture required MoonBit registry initialization and CI's generated WIT bindings;
those prerequisites now pass, as do Scala/MoonBit streaming fixture builds. Later
attempts hit disk-full during component writes and Docker image extraction. Obsolete
probe/fixture build caches were removed, sources/evidence retained, and the required
PostgreSQL image downloaded. Group runs are ongoing. Preserve these failed attempts;
do not count them as passes. The subsequent broad group1 run reached late stream tests,
then multiple tests timed out with roughly 2400 SQLite worker threads in the process;
the process ultimately exited 137 without a complete result. This remains unresolved,
not attributed to suspension or dismissed as unrelated. A separate baseline checkout
is building for comparison; the first comparison keeps the same patched runtime to
isolate Golem changes. No lifecycle fix is authorized merely by this observation.
Broader groups and miscellaneous runs, lint completion and M6 remain outstanding.
Clippy found test-only type-complexity, unnecessary-Vec and manual-noop-waker warnings;
scoped corrections/reruns are in progress.

The baseline build completed and group1 passed with the same patched runtime:
265 passed, 0 failed, 4 ignored in 398.372s (`baseline-group1.log`).
At 07:59 UTC its process had 2523 threads (2202 SQLx SQLite workers, 317 Tokio
workers) and 20935252 kB resident memory; see `baseline-resource-sample.txt` and
`baseline-group1.log` under `tmp/gol706-m5/`. This reproduces substantial resource
growth without the new Golem coordinator, but does not exonerate the shared runtime
or explain the changed branch's failed run. No lifecycle change follows from this
observation. The remaining comparison must distinguish the additional workload and
orb resource pressure from a semantic regression; the failure is not waived.
Kernel records now confirm that the changed run's exact process was OOM-killed at
roughly 24 GB anonymous RSS (`group1-oom-evidence.txt`). This establishes the cause
of exit 137, not each preceding timeout.

Oracle approved resource-bounded verification, with trajectory AT RISK, not
DERAILED: rerun the four late failures together, run the final binary's intact
baseline-common cohort at four test threads, and run its added cohort separately.
Monitor RSS, threads, cgroup limits/events and test progress without concurrent
heavy builds. Reconcile the exact final inventory across runs, including explicit
ignored cases. Repeat a monitored baseline only if needed to explain an apparent
resource difference. An unexplained increase on equivalent work or a timeout
without resource pressure remains blocking. Partitioned coverage never relabels
the original monolithic run green; no lifecycle change is authorized.

Scoped integration-test clippy with `-D warnings` passed after its final test-only
Vec-to-array correction (`clippy-test-fix.log`). The locked integration build without
the scratch dependency override passed against the published Wasmtime pin
(`published-build.log`, 15m29s). Its four late failures plus three lint-affected
input tests passed together (7/7, `published-late-and-lint.log`).

The initial common-cohort attempt stopped during Docker dependency creation with
no disk space; obsolete build products were removed. The corrected full baseline
cohort then completed with 264 passed, 1 failed, 4 ignored in 343.191s
(`published-group1-common-retry.log`). Its peak sampled RSS was about 25.8 GB with
2991 threads, no additional OOM event. Resource attribution remains open.

The failure is `duplicate_secret_policy_occurrences_are_isolated_from_leaf`:
the second promise checkpoint does not reach Suspended within its unchanged 30s
budget. It reproduces repeatedly alone on current code, while the baseline passes
alone (`published-secret-policy*.log`, `baseline-secret-policy-debug.log`). It is
therefore not dismissed as broad-suite pressure. Oracle identified a concrete
missing classification: `EntityInvocationDurability::drive_access` marks fresh
live body waits neutral, but not reconstructed body waits after the incomplete
replay-to-live transition. The diagnostic snapshot confirmed current blocked
evidence in all three Stores, the promise classified OnActivation, and two
non-root ancestor activities left Unknown. There was no empty retired-Store veto.

The ten-line correction classifies those ancestor waits only after the resolver
returns Incomplete and drops the guard before validation/terminal persistence.
Eligibility, lifecycle and timeout assertions remain unchanged; all temporary
diagnostics were removed. Existing suspension/entity units passed (50/50), and
the unchanged regression passed three runs: 8.272s, 6.921s, and the final repeat
in `replay-neutral-repeat3.log`. Evidence: `replay-neutral-fix.log`,
`bug-finder-ground-truth.log` (also includes one temporary adversarial unit, removed
after passing). Oracle conditionally APPROVED with these verification conditions
now met; bug-finder `gol706-m5-replay-neutral` run 1 clean. Overall trajectory
remains AT RISK verification, not DERAILED. The common/added cohorts and remaining
groups are running again on the corrected binary with `fixed-*` logs. M5 remains
open for resource attribution, full final-source coverage and final review; no
acceptance obligation has been waived.

The corrected group1 common cohort now passes: 265 passed, 4 ignored in 380.009s;
the added cohort passes 13/13 in 50.577s. A continuously monitored baseline repeat
also passes 265/4 in 349.128s (`baseline-group1-monitored.log`). Peak sampled RSS
is 27,847,284 KiB corrected versus 25,888,060 KiB baseline, with 3021 versus 2991
threads. Neither run adds an OOM. The endpoints differ, but matched completed-test
sets still show an excess; starting cgroup pressure also differs substantially.
Oracle judges AT RISK, not DERAILED, and requests one controlled baseline-to-final
pair with initial anon/file accounting, one-second samples and exit-time maximum
RSS. Compare growth at matched progress, not only peaks. A persistent unexplained
excess blocks closure and requires a narrowly localized follow-up/scope decision;
it does not authorize lifecycle work. No further production changes were made.

Group2 initially finished 59 passed/33 failed (`fixed-group2.log`). The generated
update-v1/v2 WASMs were identical, as were v3/v4; the earlier shared-target build
reused same-named packages without compiling the differing sources. Rebuilt all
four via an isolated target, cleaning `it_agent_update` between versions, then
forced Golem build and copy. Logs confirm each correct source was compiled;
copied v2/v4 now contain `revision_two_only` and have distinct hashes. Build log:
`rebuild-update-fixtures.log`. No source/migration changes resulted. Groups2–4
are rerunning unchanged (`fixed-fixtures-*`); individual timeouts are not declared
fixture-caused until the rerun verifies that. Retry/quota/misc and final review
remain outstanding, along with the bounded resource comparison.

The repaired-fixture group2 rerun finished 91 passed/1 failed: the unchanged
snapshot-promotion count assertion observed 2 versus 1; it subsequently passed
alone on both baseline and current code. It remains an intermittent comparison
obligation. Group3 finished 187 passed/5 ignored/1 failed. Its unchanged
`caller_recovery_restarts_input_drain_after_rpc_result_commit` passes alone on
baseline and fails alone on current code: the caller remains Running with retry
count zero. Diagnostic snapshots show valid blocked evidence in both Stores,
the caller promise classified OnActivation and input drains SourceWait, but a
non-root transfer remains Unknown in each Store. Logs: `fixed-input-drain.log`,
`baseline-input-drain.log`, `input-drain-diagnostic.log`.

Oracle identifies a missing durable-source read classification after the RPC
result has already completed. The bounded candidate exposes the existing transfer
ID during Wasmtime producer/consumer polling (initial pre-admission polls return
None), then classifies only the established `reader.next()` source observation.
Use ordinary wait grace and persisted RPC-style timed rechecks; do not infer
cross-owner input/output dependency or assume attachment activation starts a
producer. Mapping/activation, consumer journaling and delivery remain blockers.
The initial candidate incorrectly parked after Stop; Oracle rejected it because
the driver requires an existing wait to propagate typed Suspend. The revised
candidate preserves that typed interruption through a local receive-error variant,
without adding a stream terminal or changing lifecycle. Initial diagnostics are
removed. New acceptance covers both owners unloading, autonomous consumer rechecks,
independent output, exact values and a follow-up. Fork identity tests passed 5+1;
executor verification, additional race/phase coverage, bug-finder and follow-up
Oracle approval are pending. These uncommitted candidates are not published or
accepted. Oracle trajectory remains AT RISK, not DERAILED.

Revised stream-read checkpoint: the typed-interrupt candidate compiles and passes
121 selected session/suspension unit tests plus both unchanged caller/callee
recovery controls (`stream-read-typed-stop-build-fix.log`). The new independent
output acceptance case initially failed because its test promise belonged to the
caller rather than the waiting target; corrected only the test setup. Oracle
requested phase attribution: observe the matching committed RPC End and first
consumer item before counting unload, then require autonomous reload while the
promise remains closed. The strengthened test passes both input and independent
output cases, exact results, contiguous consumer ordinals and a successful terminal
in three runs (`stream-read-phase-check{,-2,-3}.log`, 9.538s/9.686s/9.585s).
Oracle source review finds no confirmed production defect and approves the bounded
approach, but holds checkpoint approval for evidence; trajectory remains AT RISK,
not DERAILED. Current fork observation suite passes all six tests, including the
new consumer identity/drop case (`bug-finder-fork-current-run2.log`). Bug-finder
run 2 resolves the undeclared-dependency finding but identifies a compile error
in the new adapter interruption test (`Interrupt` requires a timestamp). Corrected
the test constructor/patterns; its rerun and final bug-finder approval remain open.
No new production lifecycle work is introduced and this correction is not published.

The subsequent seven-test byte/nested control run found 5 passes and 2 failures
(`stream-read-byte-nested-controls.log`): the candidate incorrectly classified
open external input as suspendable. Existing resident assertions remain unchanged.
Oracle rejects an Attached-only shortcut because same-owner agent/tool streams
also use local readers. The bounded correction resolves local persisted source
registration: ExternalInlineInput and its actual Nested descendants veto; local
AgentHostedInput/InvocationOutput and their descendants may contribute a timed
recheck. Forwarded handles retain their original registration. Attached downstream
consumers may recheck committed producer history; this grants no suspension
permission to the upstream ingress owner. Missing/invalid ancestry is an existing
typed history error, never eligibility. No handle, wire or persisted format changes.
Metadata resolution precedes the classified read. Receive-path unit cases cover
roots, two nested levels, foreign-binding forwarding and reconstructed metadata;
the first combined build found two older direct reader constructors missing the
derived field; updated them using the same metadata query. Validation is running
in `stream-read-provenance-final.log`. The new interruption unit also exposed
missing test session mapping; it now uses the existing open-local-session helper.
Oracle explicitly accepts the actual nested-registration/receive matrix plus the
existing end-to-end controls instead of requiring new guest fixture APIs: isolated
descendant activity proves a parent cannot mask the negative decision, and local
reader positives reject an Attached-only shortcut. This is not a claim of a new
same-owner tool/entity consumer-read unload demonstration. Oracle finds no source
blocker and rates this classifier ON TRACK; current-candidate test results and
bug-finder approval remain required. Overall M5 resource/regression gates stay open.

**Historical mandatory pause — bug-finder run 3:** provenance-corrected code passed
125 selected session/suspension units, including all three provenance matrices,
and seven unchanged integration controls (`stream-read-provenance-controls.log`).
The interruption test still fails before its adapter assertions: opening resident
bindings does not persist a Mapping; `endpoint` correctly requires its durable
reader identity. Bug-finder confirms this new test-setup regression and resolves
the earlier tuple-variant compile finding. Its three successive findings trigger
the mandatory checkpoint; no override or further implementation has occurred.
Oracle rates the checkpoint AT RISK, not DERAILED: the shared cause is insufficient
verification of local dependency/API/fixture contracts, not demonstrated production
architecture failure. Proposed human-approved continuation is fixture-only: use
existing `persist_local_mapping`, open the same session with that returned binding,
preserve every assertion and take the no-write baseline after setup. Run the isolated
test, then the selected units, then three positive two-owner E2E runs on this exact
provenance-corrected candidate and obtain both reviews. Any unexpected failure or
need for production changes stops that bounded attempt. The earlier three positive
runs predate provenance correction. Broader M5/resource/pinned-delivery gates remain
open. This candidate remains local and uncommitted; existing draft PRs are unchanged.

**Human-approved continuation complete:** the user approved the explained fixture-only
repair. The test now persists the exact local mapping with `persist_local_mapping`
and opens the session with the same key and returned binding. All interruption,
consumer-state, no-write and no-terminal assertions remain unchanged; production
code did not change during this continuation. Isolated test passes
(`approved-fixture-isolated.log`); selected durable-session/suspension suite passes
126/126 (`bug-finder-approved-controls-run4.log`); the prior failing reproducer
passes (`bug-finder-interrupt-binding-run4.log`). Current provenance-corrected
two-owner E2E passes three runs (`approved-provenance-e2e-{1,2,3}.log`, total test-run
times 9.741s/9.944s/9.836s). Those E2E runs used the existing current-production
binary while the fixture-only unit rebuild ran, not strict sequential ordering.
Oracle explicitly accepts that ordering, APPROVES this bounded checkpoint and
rates its trajectory ON TRACK. Bug-finder run 4, with the human-authorized override,
marks the missing-binding finding RESOLVED and reports no new/recurring findings.
Its cumulative historical `designCheckpoint=true`/`clean=false` flags remain;
do not call that an unqualified clean result or start another unchanged review loop.
Plan/Linear record updated. This does not close broader M5 or authorize merge;
the stream correction and fixture repair remain local/uncommitted.

Controlled resource comparison completed on the provenance-corrected candidate:
baseline and current each pass 265 tests with 4 ignored, in 361.574s and 385.826s.
Exit maximum RSS is 26,238,516 versus 27,919,792 KiB; peak threads 3018 versus 3010;
neither adds an OOM. At 65 exact matching started-and-finished sets, late current
RSS remains approximately 1.2–1.7 GiB higher. Initial anonymous memory is comparable,
but file cache differs. Evidence: `controlled-{baseline,final}.log`, corresponding
`-resources.jsonl`, and `controlled-resource-comparison.txt` under `tmp/gol706-m5/`.
The initial disk-full attempt is preserved as `controlled-baseline-disk-full*`;
obsolete build outputs were removed before the successful pair, not source or tests.

Oracle rates M5 AT RISK, not DERAILED: this is actionable unexplained excess, not
proof of a suspension leak. Approximately 1 GiB divergence already precedes the
first test-start line, making dependency/component prewarming the first attribution
target. Proceed with an observation-only reduced cohort retaining the same tagged
component-prewarm inventory plus the service-release sentinel; capture process
anonymous/file RSS, smaps rollup, thread names and component/cache phase logs.
Run once cold/as-found and once warm per binary, without shared-cache clearing,
allocator tuning, suspension changes or lifecycle edits. Cap investigation at half
a working day plus build time. If the signal disappears or attribution remains
inconclusive, stop for a specific next-experiment decision. No new implementation
checkpoint or bug-finder run is claimed for this measurement-only work. All broader
M5 obligations and final published-pin verification remain open.

Reduced attribution experiment completed: 11 common tests covering all 15 tagged
component dependencies, including the service-release sentinel, pass in each of
four runs (baseline twice, current twice; approximately 25s each). Inventory and
evidence: `prewarm-cohort.json`, `prewarm-{baseline,final}-{1,2}.{log,jsonl}` and
`prewarm-comparison.txt`. The GiB-scale signal is absent; anonymous-memory peaks
overlap. These are as-found/repeat runs, not certified cold/warm comparisons, and
last-live samples precede completion of every test, not post-teardown measurements.
Oracle found the requested cache log target ineffective: this runtime requires
`wasmtime_internal_cache=trace`, not `wasmtime_cache=debug`. Golem's analysis/compile
messages alone do not establish Wasmtime cache misses. No production defect or
resource-gate closure follows from this experiment.

**Historical pause, superseded by the user's correctness-first decision above.**
Oracle's verdict at this checkpoint was AT RISK, not DERAILED.
Proposed follow-up, now deferred to GOL-761: exactly four invocations of the same
11-test cohort, each binary with its own initially empty absolute `XDG_CACHE_HOME`
then reusing that cache. Verify actual Wasmtime cache paths/computation events,
capture named prewarm boundary timestamps and process anonymous/file RSS, with
100–200ms lightweight startup samples and one-second smaps. No shared-cache clearing,
binary rebuild, allocator tuning or production/lifecycle changes. Cap at 10 minutes
per invocation and three hours total; stop after four runs or earlier on missing
cache evidence, failure, pressure or timeout. Positive evidence can narrow startup
cache contribution but cannot by itself close the broad resource gate. No automatic
retry, cohort expansion, heap profiling or lifecycle work is authorized by this proposal.

Group4 is incomplete: the short-sleep scalability p95 exceeded its bound before
Docker Ignite setup exhausted disk and the monitor failed. No complete result is
claimed. Disposable fixture build output and abandoned test databases were removed;
the logs remain. Baseline latency comparison and a full rerun remain obligations.
The completed resource comparison above uses the provenance-corrected candidate;
its attribution is deferred to GOL-761 and no longer gates GOL706.

Correctness-first verification resumed on the same current-source binary. Group2
passes 92/92 (71.307s); group3 passes 189 with 5 ignored (356.880s). The five ignores
require privileged managed-XFS or the dedicated filesystem benchmark runner.
The added group1 cohort passes 13/13; in-function retry 57/57; untagged 25/25;
sequential storage 583/583; oplog archive 2/2; RDBMS service 15/15; Ignite service
6/6. The configured `:tag:storage_quota` selects zero tests in this checkout and is
not claimed as coverage. Executor library tests pass 2658 with 8 ignored. The six
original regressions plus shared-scheduler scenario pass three final-source runs,
7/7 each (101.626s, 99.551s, 101.730s). Runtime observation tests pass 6/6 and host
dispatch 3/3. Evidence is in `tmp/gol706-m5/correctness-*.log`; scoped executor
formatting and diff whitespace checks pass. Final lint/published-pin checks remain.

Group4's parallel attempt again exhausted Docker VFS disk, causing harness shutdown
and secondary actor errors. Its failed log is preserved. After removing abandoned
test containers and obsolete build artifacts, a complete sequential run finishes
121 passed/1 failed in 725.532s (`correctness-group4-sequential.log`). The only
remaining failure is the unchanged short-sleep p95 bound: 7515ms against 6000ms.
Isolated comparisons fail on both baseline (6897ms) and current (8111ms). All 99
measured invocations return successfully before this assertion; the test does not
validate their payloads, and the later total-duration assertion does not execute.
Both isolated logs have a shared post-result deserialization-task cancellation panic;
do not claim the logs contain no errors or that its cause is proved. The baseline
uses the previously documented patched runtime, not an unmodified-upstream claim.

Oracle rates M5 AT RISK for incomplete closure, not DERAILED, and recommends an
explicitly accepted performance exception for this baseline-reproduced latency
failure. Candidate-specific latency overhead remains unresolved. **Historical pause,
superseded by the user's explicit approval of GOL-769 and the temporary ignore:**
memory deferral alone did not waive latency assertions. The original failure remains
evidence; the new run must report the ignore honestly.
No new production implementation checkpoint or historical bug-finder rerun was
created by these measurements. Existing final-review obligations remain.

The subsequent lint checkpoint changes only the nested activity-binding `if let`
to an equivalent let-chain in `DurableInputProducer`. Scoped clippy auto-fix passes;
formatting is applied and the full file is compared to its saved pre-fix snapshot,
confirming exactly that one hunk. Bug-finder `gol706-m5-clippy-let-chain` run 1 is
clean; five extracted-source executable cases cover absent binding/activity, first
set, same-ID repeat and different-ID rejection (`clippy-probe.log`). Oracle APPROVES
the mechanical equivalence and explicitly retains the preceding broad tests as
pre-fix functional evidence without requiring full-suite reruns for this expression.
The final nonmutating executor library/integration clippy command passes with
`--no-deps -- -D warnings` (7m44s, `correctness-clippy-final.log`); dependency warnings
remain visible. Shared-engine compatibility passes 1/1 and component extraction 5/5
against the current local fork (`correctness-engine-compatibility.log`,
`correctness-agent-extraction.log`). Final scoped formatting and diff checks pass.
**Historical state before the final delivery checkpoint below:** the latest stream-read
correction, mechanical lint fix and runtime activity API remain local/uncommitted;
the tracked lockfile contains scratch path-override drift that must not be published.
M5 closure awaits explicit latency disposition; M6 still requires final publication
of the corrected fork, matching root dependency pin/clean-checkout verification,
documentation alignment and final delivery review. No merge/release is authorized.

#### Final delivery checkpoint

The user approved GOL-769 and temporarily ignoring only
`scalability::spawning_many_workers_that_sleep`. Both original timing assertions
remain unchanged; the long-sleep suspension control remains enabled. GOL-761 and
GOL-769 are accepted follow-ups, not open suspension-fix gates.

The fork activity API is published as
[4ff92c67](https://github.com/golemcloud/wasmtime/commit/4ff92c67b5eee08ff94460d6544c5d7a5c7ae0bf).
All four manifest patches and all 35 lockfile source records match; no package
version or dependency drift remains. `cargo test --locked -p golem-worker-executor
--test integration --no-run` with debug information/incremental disabled and four
build jobs succeeds in 16m58s (`delivery-published-build.log`), without scratch
overrides. The full sequential group4 rerun finishes **120 passed / 1 failed /
1 ignored** in 853.167s (`delivery-group4.log`). Dynamic-memory allocation hits
its unchanged 240-second timeout; short-sleep is the authorized ignore. The
long-sleep suspension control passes in 46.346s. This is not a green group run.

One bounded isolated execution of the same published-pin binary passes both
`scalability::dynamic_large_memory_allocation` (234.668s) and
`rpc::durable_rpc_stream_reads_unload_and_recheck_both_owners` (9.528s), with
unchanged assertions/timeouts (`delivery-isolated-controls.log`, 2/2 in 244.597s).
Commands, from `golem-worker-executor/` after the locked build:

```shell
../target/debug/deps/integration-b68337a3ad7ce006 ':tag:group4' --test-threads=1 --report-time
../target/debug/deps/integration-b68337a3ad7ce006 'scalability::dynamic_large_memory_allocation' 'rpc::durable_rpc_stream_reads_unload_and_recheck_both_owners' --exact --test-threads=1 --report-time --nocapture
```

Oracle explicitly approves M5 correctness closure and draft delivery, ON TRACK,
revising the earlier expectation of a clean single group4 run. The timeout is not
proved environmental: suite-order, scheduling/retry sensitivity and intermittent
liveness remain possible. The isolated pass has only 5.332s headroom. Keep this
residual verification risk visible in GOL-761/GOL-769 and the draft; no second ignore,
assertion relaxation, CI bypass or lifecycle work is approved. No additional
repeat-until-green run is required. The reviewed implementation is now committed
and pushed as [afa31b101](https://github.com/golemcloud/golem/commit/afa31b101), with
both draft PR descriptions updated to these exact results. Merge/release remain
unauthorized, and required CI gates are not bypassed.

Final incremental bug-finder `gol706-final-delivery` run 1 is clean. Oracle approves
the current production diff and suspension-only scope, rates progress ON TRACK,
and requires only record/publication updates after the executions above. Prior
reviewed broad suites are source-equivalent evidence, not falsely
relabeled as executions against the new Git source. Documentation now records
exact stream transfer attribution and the external-input veto; desktop and narrow
Chromium captures were inspected, all three cards readable without overflow.

Historical M6 documentation checkpoint: Oracle APPROVED, bug-finder `gol706-m6-docs` run 1
clean. Desktop and narrow Chromium captures were inspected; all three narrow
owner-suspension cards are complete and readable. Oracle trajectory remains
AT RISK for unresolved M5 verification, not DERAILED. Final M6 delivery remains open.

#### Draft publication requested during verification

The user explicitly authorized committing all current changes and pushing draft PRs,
then continuing the existing investigation. The Golem checkpoint is
[`c3c207ee`](https://github.com/golemcloud/golem/commit/c3c207eed)
on `gol706-owner-suspension`, published as draft PR #4062. The runtime commit is
[`b0db5fed`](https://github.com/golemcloud/wasmtime/commit/b0db5fedc9cdfc4b72238e7d5b08e875abcb56a0)
on `gol706-suspension-observation`, targeting `golem-wasmtime-v46.0.1-p3` in draft
PR #9. Root Cargo metadata resolves all four Wasmtime patches to this published pin
without the scratch configuration (`published-pin-metadata.json`). Full verification
through the published dependency source remains required. Draft publication is a
checkpoint, not approval to merge, release, or declare the broad-suite failures fixed.

#### M5 API timeout adjustment: approved

`api::get_workers_opaque_cursor_replays_after_restart` passed alone in 6.508s but
failed in the parallel API suite waiting 10s for Suspended (last status Running).
The owner checks after 1s then every 10s; enumeration may still be active at the
first check. The test budget is now 30s, with all actual suspension, replay, page
contents and exact enumeration-count assertions retained. No production change.
Adjusted parallel API suite: 83 passed, 0 failed, 2 ignored; affected test 17.995s
(`api-parallel-adjusted.log`). Whole-test duration does not prove exact check timing
or measure suspension latency. Original logs: `opaque-cursor-targeted.log` and
`api-parallel.log`. Oracle approved this adjustment and adjudicated the API failure;
bug-finder `gol706-m5-api-timeout` run 1 clean. Remaining M5 checks are unchanged.

### M4l — input and P2 file acceptance; M4 closure: DONE

Oracle APPROVED / ON TRACK for M4l and cumulative bounded A1–A7 closure;
bug-finder `gol706-m4l-input-file-acceptance` run 1 clean. No production or lifecycle
change in this checkpoint. M4 is complete; do not infer M5 or release completion.

`tmp/gol706-m4/m4l-final-integration.log`: 5 passed, 0 failed. Selectors:
`consumed_scalar_input_survives_automatic_unload`,
`consumed_byte_input_survives_automatic_unload`,
`consumed_nested_input_survives_automatic_unload`,
`generated_input_source_timer_suspends_in_flight_transfer`,
`p2_file_pollables_ready_and_block_survive_full_replay`.
Both selected fixtures rebuilt; logs `m4l-{file,input}-fixture.log`.

External input tests observe committed consumption before withholding input, prove
Running/residency/no Suspend while it remains open, then finish consumption and
observe actual timer-driven unload before invocation completion. Reconstruction
preserves every input value/terminal/offset by durable source identity and logical
ordinal; consumed history is unchanged. The generated RPC source test suspends the
caller during an unfinished transfer and matches the target's exact prefix and final
consumer journal against the caller's producer journal, then invokes both agents.
The P2 fixture unconditionally calls file pollable block/ready and actual asymmetric
stream read/write; two full reconstructions preserve append contents and follow-ups.

Oracle requested stronger negative-window and exact-association assertions after
the first corrected five-test pass; these are present in the final five-test pass.
Initial four input failures incorrectly counted constructor completion, now filtered
to the tested method. Logs preserved as `m4l-integration.log` and
`m4l-corrected-integration.log`; only the final snapshot closes acceptance.

| Row | Decisive cumulative evidence (logs under tmp/gol706-m4 unless noted) |
|---|---|
| A1 | tmp/gol706-m3/structured-wake-regression.log and final-timer-races.log: short/multiple watchdog refusal, polling progress, all-long unload/earliest deadline, persistence invalidation |
| A2 | promise-combined-final.log and promise-handoff/repeat2/repeat3: ready-before, pre/post-unload completion and restart, exact payload/key |
| A3 | p2-final-gate.log, m4i-reviewed-integration-available.log, m4i-filesystem-final.log and m4l-final-integration.log: timer unload, both HTTP orders, poll duplicates/lifetime, full filesystem and unconditional file pollable replay |
| A4 | owned-rpc-final-integration.log, rpc-resident-controls-final.log and m4h-final-baseline.log: owned async grace/recovery/one-slot/keyed effect and borrowed veto |
| A5 | entity-integration.log and m4h-final-baseline.log: tool/entity aggregate eligibility, runnable siblings/native veto, unload and follow-up |
| A6 | m4j-reviewed-units.log (128), m4j-reviewed-integration.log (6), m4k-final-integration.log (8), m4l-final-integration.log (5); named generated-client/nested-restart passes in initial m4k-integration.log: post-result and scalar/byte/nested input/output, in-flight owned source, exact journals |
| A7 | m4k-final-integration.log and named persistence/handoff passes in initial m4k-integration.log; stale-generation accounting units in m4j-reviewed-units.log: wake handoff, interruption/shard priority, deadline, quota/fuel |

This is representative contract coverage, not every possible instruction-level
interleaving. Unresolved external input stays resident; its tests close input before
timer suspension. The generated owned-source case covers complementary unfinished
transfer. Startup fuel denial is not a mid-invocation replenishment claim. M5 must
still repeat original regressions on final source and execute broader verification.

### M4k — byte/nested prefixes and mandatory-stop acceptance: DONE

Oracle final APPROVED / ON TRACK; bug-finder
`gol706-m4k-stream-boundary-acceptance` run 1 clean. This checkpoint adds acceptance
tests and two fixture methods only; no production or lifecycle change.

Final combined execution: 8 passed, 0 failed in
`tmp/gol706-m4/m4k-final-integration.log`. Use the root Cargo environment and local
fork config recorded below, `--test integration`, with these OR selectors:
`durable_byte_output_prefix_survives_autonomous_suspension`,
`durable_nested_output_prefix_survives_autonomous_suspension`,
`automatic_schedule_persistence`, `invocation_deadline_unwinds_parked_p3_wait`,
`zero_fuel_suspends_and_unloads_runnable_constructor`,
`dynamic_tool_invocation_quota_exhaustion_during_activation_resumes`,
`dynamic_tool_http_limits_apply_before_dispatch`.
Bug-finder's concurrent six-test rerun also passes in
`m4k-adversarial-parallel-review.log`. Scoped rustfmt and diff checks pass.

- Byte prefix `[23,169]` and nested child prefix `[169]` are committed before actual
  automatic unload. The same invocation session receives the exact suffix `[7,201]`
  after autonomous reconstruction, with no demand/resume to rescue it. Every logical
  value is matched by durable stream identity and sequence to its journal value;
  each wire frame's offset equals its final covered item's offset. Nested identity,
  terminal sequence/offset/Ok, no cancellation, and follow-up ping are checked.
- Explicit interruption and real shard revocation complete while automatic wakeup
  insertion is held; the old primary unloads before releasing that gate. Replacement
  identity and exactly one keyed invocation completion are checked. This is not a
  claim about every late-callback race.
- A real five-second invocation deadline unwinds a resident sixty-second P3 wait,
  without successful wait completion or voluntary suspension.
- Production zero-fuel startup unloads before constructor completion; existing
  activation and HTTP quota tests preserve their mandatory suspension behavior.
  This proves startup fuel denial, not mid-invocation fuel depletion/replenishment.

Initial failures were acceptance-test errors, preserved in `m4k-integration.log`
and `m4k-corrected-integration.log`: packed bytes are not single-item frames; the
transport pumps child streams after the parent; public oplog queries require shard
ownership; start indices are inclusive; uncommitted Start entries are not public
observations. Oracle required moving nested observation to the enclosing mapping,
checking its child prefix in committed storage while unloaded, and strengthening
set-only offsets to exact stream/sequence/value correspondence. No assertions were
relaxed to hide a production failure. Initial combined run also passed nine retained
controls, including generated-client streaming and nested sibling restart recovery.

M4 remains ACTIVE until A1–A7 evidence is reconciled; M5 final-source repeated
regressions/broader suites and M6 docs/reproducible delivery remain. All work is local,
uncommitted and unpublished.

### M4j — post-result output suspension: DONE

Oracle final APPROVED / ON TRACK; bug-finder
`gol706-m4j-post-result-streams` run 1 clean.

The existing post-result runtime driver now participates in owner suspension.
Live output endpoints carry source-Store accounting identity, not Store ownership.
Drains and queued nested children register before handoff and remain Unknown during
preparation, attachment, encoding, journaling, batching and publication. Only an
empty, pending receive from an accounted source can become passive; it contributes
no deadline/activation by itself and never hides other runtime or external work.
Unknown network/transfers still veto. Projection relays remain conservative.

Review-driven corrections, all within suspension accounting:
- Receive invalidation is shared across polls and replacement receives, so an old
  saved waker cannot be overwritten by a newer Pending publication.
- Data receives stop polling after commitment but retain custody until existing
  cancellation/teardown. The already-admitted child coordinator continues polling
  cleanup joins; it cannot start an unregistered child after commitment.
- Output mutation admissions retain their own external veto after caller drop,
  through status/publication tails. Scoped source capture is installed per drain;
  detached operations retain the guard in existing admission ownership.
- Lifecycle publication receipts retain only the accounting charge, not admission
  permits or the producer. The producer is dropped at its original completion
  boundary. No spawn/abort/join/cancellation ownership or teardown protocol changed.

Evidence under `tmp/gol706-m4/`:
- `m4j-reviewed-units.log` and `m4j-bug-finder-baseline.log`: 128 passed, including
  actual coordinator cancellation after commit, queued-child veto, old-waker and
  commit-order races, and detached mutation/callback/publication custody. Mutation
  tests assert owner eligibility, not reference counts; deferred terminal coverage
  also verifies producer release while the accounting veto remains.
- Bug-finder `gol706-m4j-post-result-streams` run 1 clean. Its isolated threaded
  saved-waker probe and 33 existing policy tests also passed (`m4j-review-race*.log`).
- First real integration `durable_output_producer_suspends_after_result_and_autonomously_resumes`
  passed in 22.403s (`m4j-post-result-first.log`): early result, actual unload,
  autonomous reconstruction, exact `[23,169,7]`, durable offsets, one terminal and
  invocation completion, follow-up. This predates the final accounting corrections.
- `m4j-reviewed-integration.log`: 6 passed on final source in 30.078s. Post-result
  unload/autonomous reconstruction passed again with exact values/offsets/terminal
  and follow-up; durable, ephemeral and replayed output interruption passed;
  durable input/output executor restart controls passed.

M4/A6 remains open: this slice does not establish every stream direction, bytes,
non-flat/nested suspension or all race cases. Broader A7, M5 and M6 remain required.
No new fork changes in M4j. All work remains local, uncommitted and unpublished.

### M4i — local-election removal: DONE

Oracle final APPROVED / ON TRACK; bug-finder
`gol706-m4i-local-election-removal` run 2 clean. Durable fallback waits cannot
elect suspension; borrowed P2 polling performs one native wait without dropping
and recreating it. Native suspension callbacks remain enabled only for ephemeral
contexts. Receive custody continues through journal persistence and nested
materialization. Removed obsolete synchronous marker probes, not asynchronous
draining or ownership. No lifecycle or fork changes in this checkpoint.

Final evidence under `tmp/gol706-m4/`:
- `m4i-reviewed-units.log`: 54 passed (53 retained tests and one subsequently
  removed provisional marker-cancellation hypothesis test).
- `m4i-reviewed-integration-available.log`: 10 passed, covering P2/HTTP schedules,
  repeated/duplicate polling, file timestamps and mixed-owner vetoes.
- `m4i-filesystem-final.log`: full mutation-history reconstruction and
  cross-preview append replay both passed after rebuilding the missing fixture.
- `m4i-rereview-hypothesis.log`: 11 passed; marker receipt/permit loss did not
  reproduce. This overlaps existing evidence, not additional acceptance coverage.
- Scoped formatting and `git diff --check` passed.

Adjustment: restored the missing initial-filesystem fixture before closing the
gate. Remaining: A6 stream/post-result acceptance, broader A7 races, M5 final-source
regressions and M6 reproducible delivery. M4 remains ACTIVE. All changes remain
local, uncommitted and unpublished.

M4h final evidence: `tmp/gol706-m4/m4h-final-baseline.log` (2 passed in 68.948s),
`m4h-native-repeat.log` (1 passed), `m4h-provisional-delayed-cancel.log` (1 passed
under temporary delayed cancellation, restored afterward). The renamed M4g
two-Store invalidation test also passed in `m4g-neutral-renamed.log`.
The borrowed fixture explicitly polls its P3 timer before entering synchronous RPC.
The native fixture runs the actual Host backend with no streams, uses a suspendable
promise to trigger cancellation, and actively awaits its result beside the timer.
Oracle's requested refinements require the specific Cancelled result, an elapsed-time
residency window, and no repeated live effect after replay and follow-up. The native
counter counts live effects, not replay entries into the body. No production logic
or lifecycle ownership changed for M4h.

M4g accounts entity activity before the existing Tokio spawn, leaves admission,
native execution and finalization Unknown, and delegates only the component's
actual runtime driver to its independently checked Store. Parent result waiters
reference the unfinished ToolExecutionTask activities; live body joins reference
the separately registered entity. Neither internal wait supplies a wake deadline.
Detached replay supervisors/monitors and executing retained-resource cleanup are
conservative blockers. No task ownership, cancellation or teardown policy changed.

The first draft failed the scalar long-timer integration: it classified the entity
instead of its parent waiter. This was corrected before closing the checkpoint.
Oracle also found that completed producers must be removed from aggregate wait
dependencies; the correction has a batched 20s/2s production regression requiring
exactly one entity terminal before unloading. New runtime tests exercise real
driver wake, return and drop, plus nested neutral waits and unknown dependencies.
Unit evidence: `tmp/gol706-m4/provisional-m4g-review-unit.log` (75 passed, including
tool-operation controls). Final production evidence:
`tmp/gol706-m4/entity-integration.log` (4 passed in 79.238s): scalar long timer,
aggregate partial completion then suspension, clock-race residency, native scalar
admission. Oracle's final review approved this bounded checkpoint and judged ON
TRACK. Task ownership, cancellation and teardown remain unchanged. M4 stays ACTIVE.

M4e evidence: `tmp/gol706-m4/rpc-resident-controls-final.log` (5 passed) and
`tmp/gol706-m4/rpc-resident-bugfinder-followup.log`. The initial residency check
incorrectly required a buffered caller Start to appear in the public persisted
oplog before completion. Logs proved dispatch and callee execution had occurred.
The corrected pending check observes one wrapped RPC attempt; final exact result,
callee execution count, idempotency keys and absence of caller Suspend remain checked.
This is a test observation correction, not a production persistence change.

M4f adjustments stay inside suspension accounting: external task publication and
wake/revoke transitions now share lock ordering; expired persisted RPC recheck
deadlines cannot commit; cancelled remote phases clear passive classification;
background tasks subscribe directly to typed owner stop. No TaskScope, task
ownership or executor teardown changes. Final evidence under `tmp/gol706-m4/`:
`owned-rpc-final-unit.log` (21 passed) and `owned-rpc-final-integration.log`
(7 passed). Tests cover 1000 concurrent publication/wake/revoke races, nonzero
grace on a new remote phase, expired persisted deadlines, direct stop wake and
application drop, observed and unobserved RPC suspension, synchronous residency,
completed replay, HTTP contention and streaming input/output restart.

The initial unobserved-result timeout was a test-policy mismatch, not a missing
activity: diagnostic output showed RPC/timer/root blocked evidence. The first
1s policy check could precede 1s RPC grace; the next default 10s recheck exceeded
the test's 10s timeout and left less than the minimum 10s on its 20s timer. The
test explicitly configures 100ms rechecks; production defaults and all unload,
autonomous-resume, exact-effect and follow-up assertions are unchanged. Oracle
confirmed this correction; the first bug-finder finding was rejected on that
evidence and the corrected final-source test passes.

## M4d mechanism gate — retired by approved contract change

The user approved non-suspending borrowed RPC waits after this investigation.
The proposed follow-up probe below is historical and will not run. Its INCONCLUSIVE
result stands; the new contract removes the need for a positive borrowed-wait witness.
Negative owner-veto tests and async production acceptance remain required.

Direct RPC and target-activation waits retain `&mut DurableWorkerCtx` through borrowed
host calls. They yield inside Wasmtime's `KeepStore` fiber path, before the current
driver can publish its no-work observation. The accessor result-get path does not
remove those waits. A blanket accessor port is not a binding-only change: existing
authority capture, indexed request construction, incomplete replay and in-function
retry APIs also need preservation. No such port or lifecycle change was started.

The scratch fork under `tmp/gol706-m4-rpc-probe/wasmtime` starts from the exact owned
fork base/diff. Only a probe test was added. Both test binaries passed (4 dispatch,
4 inherited observation tests). The new test covers async and truly synchronous
enclosing exports: first borrowed Pending has no `DriverBlocked`, an admitted
AccessorTask is unpolled, and after readiness/manual repoll the exact result returns
while that sibling remains unpolled. Logs/patches/revisions are in
`tmp/gol706-m4-rpc-probe/evidence/`; corrected report is `RESULT.md` beside it.

Limits: no borrowed-alone case, no previously-polled sibling becoming ready, no
wake-forwarding proof (initial noop waker), no candidate observer extension, and no
production RPC unload/replay. The initial NO-GO wording overstated the evidence;
Oracle required **INCONCLUSIVE / AT RISK**, not proof that a minimal change is
impossible. Bug-finder is clean for the corrected bounded evidence, not for an A4
implementation. The plan's inconclusive-mechanism stop rule applies.

Proposed approval: one further scratch-only checkpoint capped at one working day,
with policy-free borrowed-import/carrier observations and selected-work accounting,
reusing existing wake/generation machinery. No scheduling-order changes, release of
a retained Store, lifecycle/RPC durability refactor, or permanent Unknown exclusion.
Do not promote the patch without Oracle and bug-finder review.

Fixed matrix, in order:
1. A previously-polled sibling import wakes while borrowed import stays pending:
   distinguish correct invalidation from actual sibling progress. If a required A4
   transition needs Store release, stop and present that precise port decision.
2. Borrowed-alone positive blocked witness and exact import/carrier identity under
   both export ABIs.
3. Admitted/unpolled and selected/unexecuted siblings remain accounted blockers.
4. Counting waker and gated publication prove wake-before/during-publication
   invalidation; an unrelated repoll cannot erase outstanding ready work.
5. Readiness, self-wake/yield, cancellation/drop and replacement-run callbacks do
   not produce stale evidence; existing observer tests remain green.

PASS requires a candidate with positive and negative evidence without behavior
changes. FAIL or INCONCLUSIVE (including the cap) pauses again. Even PASS is runtime
feasibility only: grace/recheck, single-slot progress, HTTP blocker completion, stable
RPC keys, exactly-once callee execution and all remaining A4/A6 production gates stay
required. M5/M6 remain unstarted; all implementation is local/uncommitted/unpublished.

## M4c promise handoff acceptance record

`suspension_races::promise_completion_during_suspend_handoff_is_not_lost` uses the
existing key-value fault decorator to hold the forced suspended-status cache flush.
The status is already folded from a committed Suspend and the Store is still loaded.
Promise completion returns (including activation) before releasing that flush. The
test observes Idle and an increased instance-load count before any reinvocation,
then checks exact `[2, 7, 1, 8]`, one Started/Finished and a fresh follow-up.
No production change or new lifecycle hook was needed.

Same bases/build environment/override as M3. Command `--test integration --
promise_completion_during_suspend_handoff_is_not_lost --report-time` passed three
times; logs `tmp/gol706-m4/promise-handoff{,-repeat2,-repeat3}.log`, test times
2.371s/2.401s/2.351s. Oracle **APPROVED / ON TRACK**; bug-finder run 1 **clean**.
This closes the missing A2 committed-suspend/pre-unload window together with M4b,
not every A7 lifecycle interleaving. Other M4 coverage and final-source reruns remain.

## M4b promise integration record

Durable promise waits now register their actual accessor activity with the owner
coordinator. Promise-only suspension relies on existing persisted promise completion
and owner activation, without inventing a timer. Mixed waits retain the earliest
timer deadline; short timers and unclassified work prevent suspension. Timers and
promises share one classified-wait guard, readiness/cancellation invalidation and
interrupt-before-committed-suspension-before-readiness priority. Ephemeral/suppressed
paths retain existing behavior. No lifecycle or Store-disposal changes were made.

Oracle requested two bounded corrections: registration errors use the durable
session's `handle.trap`, and the after-unload test waits for Idle before making any
result-fetching invocation, so that fetch cannot rescue a lost activation. Both
are applied; the final combined integration run includes them.

On the unchanged M1 Golem/fork bases and local override from M3:
- `--lib -- worker::suspension --report-time`: **12 passed**, in
  `tmp/gol706-m4/promise-coordinator-final.log`.
- `--test integration -- promise_completed_before_wait_returns_without_suspending
  promise_completion_after_unload_resumes_with_exact_data
  p3_promise_suspend_survives_executor_restart
  p3_short_timer_blocks_suspension_while_promise_is_pending
  p3_all_long_timers_suspend_and_resume_at_earliest_deadline
  wasi::sleep_longer_than_suspend_threshold --report-time`: **8 passed** in 61.702s,
  `tmp/gol706-m4/promise-combined-final.log`. The new tests check exact unequal
  payloads, same-key deduplication, one Started/Finished and a fresh follow-up.
- Oracle: **APPROVED conditionally / ON TRACK**, final tests and bug-finder required;
  those conditions passed. Bug-finder run 1: **no bugs found**.

At this checkpoint A2 still required deterministic completion during the final
suspension handoff; M4c above supplies that evidence. Other M4 rows remain open.
All changes remain local, uncommitted and unpublished.

## M4a narrow P2 production-gate record

Finite P2 timer subscriptions now have executor-owned deadline sources. The concurrent
poll/block branch uses the same deadline as borrowed readiness, the ordinary accessor
durable session, and the M3 coordinator. Unknown/mixed I/O retains its original borrowed
future; it is not canceled and recreated to extract readiness. Resource deletion clears
the timer map only after successful table deletion. No file-source rewrite was made.

The small policy-free `func_wrap_dispatch` addition in the owned fork selects before
constructing a borrowed future. A selector preference for concurrent execution is
constrained by the nearest guest task's blocking permission. Synchronous enclosing
exports retain borrowed execution, including pending and immediately-ready calls.
This corrected the raw prototype's CannotBlockSyncTask limitation without changing
the scheduler or executor lifecycle. Three permanent fork tests pass.

Oracle also identified a zero/past timer readiness regression under exhausted Tokio
cooperative budget. Already-due sources now return immediately; the permanent unit
test demonstrates raw Tokio Sleep pending under that budget while timer.ready returns
ready. Poll/block fairness still yields intentionally.

Final evidence under `tmp/gol706-m4/`, same bases and local override as M3:
- Fork `cargo test -p wasmtime --test func_wrap_dispatch`: 3 passed,
  `func_wrap_dispatch.log`; scoped format/diff checks pass.
- Executor `--lib -- past_timer_ready_is_immediate_even_without_cooperative_budget
  --report-time`: 1 passed, `past-timer-ready.log`.
- Executor `--test integration -- wasi::sleep_longer_than_suspend_threshold
  p3_request_completes_while_blocked_in_p2_sleep_past_suspend_threshold
  p2_sleep_completes_while_p3_request_remains_pending
  p2_poll_preserves_duplicate_indices_and_repeated_readiness --report-time`:
  6 passed in 87.287s, `p2-final-gate.log`. Includes actual P2 long-sleep unload/replay;
  HTTP5s/P2sleep15s and HTTP16s/P2sleep12s with exact result, one effect, follow-up and
  suspension bounds; duplicate/repeated polling returns `[1, 2];[0, 1];[0];[0];[0]`.
  Two substring-matched sleep-during-request controls use P3 timers and are explicitly
  not counted as P2 coverage.
- Host-api fixture rebuilt from current source using the M1 release build/copy workflow.
  No tracked fixture migration changes were generated by this rebuild.
- Oracle follow-up: ON TRACK/no blockers for this bounded slice, conditional on final
  combined rerun (now passed). Bug-finder run 1 clean.

Both mixed fixtures initiate HTTP before entering the synchronous P2 guest call; they
prove opposite completion orders, not P2-first guest initiation or independent Rust
task execution while a Rust stack is synchronously blocked. Oracle accepted that bound.
This does not close A3: file readiness/replay, general read/write/blocking coverage,
unknown-source borrowed liveness and removal of local elections remain. It also does
not complete M4 or permit disabling suspension for supported cases.

## M3 implementation record

The focused coordinator lives in `worker/suspension.rs`. It aggregates per-Store
runtime evidence, classifies P3 timers, persists the earliest deadline, and rechecks
the same revision before notifying timer imports to return the existing timestamped
Suspend. Reconstruction replaces only derived observation state at the existing
`install_replay_generation` boundary. No teardown, cleanup barrier or new recovery
path was added. Other local elections remain M4 obligations.

Driver and policy use separate wake registration within one structured future.
A prior `select!` version invalidated its own idle proof whenever persistence woke;
the all-long positive test caught this and now passes. Oracle required active-poll
exclusion before the empty-Store exemption and interrupt-before-suspension priority;
both are corrected. Retired-run observations are rejected before mutating the current
run. Oracle follow-up approved the bounded slice as ON TRACK, conditional on the
final unit/integration tests and bug-finder; all three conditions now passed.

Executed with the M1 build environment and `--config tmp/gol706-m3/patch.toml`,
which selects the local owned fork (not a deliverable dependency pin):
- `--test integration -- p3_all_long_timers_suspend_and_resume_at_earliest_deadline
  p3_short_timer_blocks_suspension_with_long_watchdog
  p3_short_timer_blocks_suspension_with_multiple_long_watchdogs
  p3_sleep_suspends_and_resumes p3_polling_loop_completes_without_watchdog_suspension
  --report-time`: 5 passed; `structured-wake-regression.log`.
- `--lib -- worker::suspension --report-time`: 7 passed; `adapter-tests.log`.
  Tests cover first-poll siblings, asymmetric deadlines, invalidation between prepare
  and commit, old runs, cancellation and separate coordinator generations.
- `--test integration -- suspension_races interrupt_while_parked_in_p3_sleep
  p3_all_long_timers_suspend_and_resume_at_earliest_deadline --report-time`:
  4 passed; `gated-races-corrected.log`. The two storage-gated tests drive the actual
  invocation and gate only CompletePromise scheduling, before/after real insert.
  Timer readiness wins while scheduling is pending, with zero Suspend and a follow-up.
- Patched shared-engine compatibility passed in `precompiled-components-2.log`.
- Bug-finder `gol706-m3-timer-slice` run 1: clean. Its strongest hypothesis exercised
  the corrected real storage races (`bug-finder-strongest.log`). This approval does
  not cover subsequent test additions or close the remaining milestone evidence.

All logs above are under `tmp/gol706-m3/`. Initial `suspension-races.log` is rejected
as evidence: that version did not poll the invocation while waiting for an unfiltered
scheduler gate and could intercept unrelated work. Its later one-Suspend assertion
did not prove the requested race. The corrected tests replace that assertion.

Final closure evidence on unchanged Golem/fork bases `accae0e435b5097cd1d7940a5c1f568bde8a055a`
and `252ab61f67fc16e49c83575e8477a8c4eca13d0b` plus local changes:
- `--lib -- worker::suspension --report-time`: **9 passed**, `adapter-priority.log`.
  Clocks calls the tested `TimerWait::wait`; simultaneous interrupt, committed stop,
  and timer readiness preserve the Interrupt kind used by invocation deadlines.
- The integration selector above now runs **5 tests, all passed** in 72.050s,
  `final-timer-races.log`. Added `resume_during_persistence_survives_suspend_handoff`
  forces real RPC activation while the real persisted insert return is gated;
  primary instance load count increases within 5s, before the 12s timer deadline,
  followed by exact result, successful follow-up and exactly one Suspend.
- Oracle: **APPROVED conditionally / ON TRACK**; conditions were these 9/5 passes
  and updated bug-finder. Bug-finder run 2 is **clean**, no open findings.
- Scoped rustfmt and `git diff --check` pass.

The first forced-resume test required a second schedule, and failed despite logging
immediate reconstruction: remaining time can legitimately fall below the suspension
threshold. The final assertion observes reconstruction itself, not rescheduling.
This is bounded early-activation and ordinary late-deadline coverage, not every
activation position during unload. Broader A7 schedules and actual configured
invocation-deadline integration remain M4/M5, as accepted by Oracle; no lifecycle
refactor is authorized or required by this checkpoint.

## M2 implementation record

Owned fork: `/tmp/gol706/wasmtime`, base `252ab61f67fc16e49c83575e8477a8c4eca13d0b`.
Separate archived probe fork under `tmp/gol706/wasmtime` is not the implementation.
Changes remain local/unpublished. Runtime policy is unchanged: observation only plus
the final-root queued-work progress correction. Observer installation precedes work;
opaque identities cover root/import/background/transfer and queued dispatch; queued
guards survive priority promotion, selection, yielding and drop. Per-run generation
watermarks reject stale blocked events. Store-retaining/unknown work is not permission
to suspend. The Golem consumer must exclude active outer polls and invalidate decisions
on activity changes. No cleanup barrier, stop fence or lifecycle redesign was introduced.

Oracle review removed an unnecessary and inaccurate completed-versus-dropped cause bit.
Its removal exposed an early background-guard drop; the permanent lifetime regression
failed before the fix and passes after the queued future retained its guard. Lifetime
is checked before polling, while pending, after driver drop and after Store drop—not
just final balanced totals. Logs `tmp/gol706-m2/background-lifetime-{before,after}.log`.

Commands from the owned fork, using `CARGO_TARGET_DIR` resolved to the Golem target cache,
`CARGO_BUILD_JOBS=6 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`:
- `cargo test -p wasmtime --lib --features component-model-async runtime_observation_tests`
  — 3 pass (`unit-final.log`): delayed stale publication after saved wake, actual
  low-priority driver-yield cancellation, and admission/selection guard lifetime.
- `cargo test -p wasmtime --test runtime_observation --features component-model-async`
  — 3 pass (`background-lifetime-after.log`): background lifetime, fairness/final-root/
  host-to-host transfer, host-to-guest future transfer.
- `cargo fmt -p wasmtime -- --check` and `git diff --check` pass.
The saved raw probe also exercised imports, Store-retaining work, wake invalidation
and balanced IDs (`probe-m2.log`); permanent tests are the maintained acceptance surface.
`precompiled-compatibility.log` tested the original locked fork only. Patched-fork
compatibility must run when M3 integrates dependencies; do not count the old result.
Guest-to-host transfer admission is source-reviewed; full direction/value/replay
coverage remains explicitly required in A6, as accepted by Oracle for M2 closure.

## M1 acceptance record

Integration base remains Golem `accae0e435b5097cd1d7940a5c1f568bde8a055a`, locked
Wasmtime `252ab61f67fc16e49c83575e8477a8c4eca13d0b` (46.0.1). No local fork override.
On 2026-10-05 PR4034 head `de1a42fe0c4468e1a081c8db07d9a5c63f18c69c` and PR4038 head
`afa65199ac41bee91c6d048a2d5172cb3fe2c696` were open. Neither supplies suspension
observation nor is a prerequisite. Do not combine these overlapping unmerged heads.
Revalidate against an exact merged main revision if the integration base changes.

Toolchain: rustc 1.98.1 (48a229cea), cargo 1.98.1 (797e8a9bc). Build/test environment:
`CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=6`.
Built CLI with `cargo build -p golem-cli --bin golem-cli`; resolved its path using
`cargo metadata --no-deps --format-version 1`. From each fixture directory ran
`golem-cli --preset release build --yes --skip-check`, then
`golem-cli --preset release exec copy`. Tool fixtures use local `GOLEM_RUST_PATH`.
Logs: `tmp/gol706-m1/{build-golem-cli,build-host-api-tests,build-tool-streaming,copy-tool-streaming}.log`.

Run selectors below with `cargo test -p golem-worker-executor --test integration --
<selectors> --report-time --nocapture`, using that environment and saving full output.
Test module names below omit the test-r binary prefix.

| Executed selector | Fixture | Baseline result / log under tmp/gol706-m1 |
|---|---|---|
| `wasi::p3_short_timer_blocks_suspension_with_long_watchdog` | host-api-tests Clock | Premature suspension / affected-and-controls-retry.log |
| `wasi::p3_short_timer_blocks_suspension_with_multiple_long_watchdogs` | host-api-tests Clock | Premature suspension / affected-and-controls-retry.log |
| `wasi::p3_short_timer_blocks_suspension_while_promise_is_pending` | host-api-tests Clock | Premature suspension / affected-and-controls-retry.log |
| `wasi::p3_all_long_timers_suspend_and_resume_at_earliest_deadline` | host-api-tests Clock | Timeout at 30s / affected-and-controls-retry.log |
| `tool_streaming::clock_races_complete_through_tool_entity_without_suspending_owner` | tool-streaming caller/provider ClockRace | Timeout at 120s; polling-loop-vs-watchdog reached, then Suspend/reconstruction stall / clock-races-tool-baseline.log |
| `wasi::sleep_longer_than_suspend_threshold` | host-api-tests Clock | PASS, including actual unload / affected-and-controls-retry.log |
| `wasi::p3_sleep_suspends_and_resumes`, `wasi::p3_resuming_sleep` | host-api-tests Clock | PASS / affected-and-controls-retry.log |
| `wasi::sleep_longer_than_suspend_threshold_while_awaiting_response` (also matches `_2`) | host-api-tests Clock | Both PASS / affected-and-controls-retry.log |
| `wasi::p3_request_completes_while_blocked_in_p2_sleep_past_suspend_threshold` | host-api-tests Clock | PASS / affected-and-controls-retry.log |
| `tool_streaming::recorded_monotonic_clock_replays_across_incomplete_stream_recovery`, `tool_streaming::incomplete_monotonic_clock_recovers_after_completed_stream` | tool-streaming provider | Both PASS / tool-clock-controls.log |
| `wasi::p3_polling_loop_completes_without_watchdog_suspension` | host-api-tests Clock | Intermittent baseline failure: initial pass; bug-finder repeats 3 passes/2 timeouts with Suspend then replay stall / primary-loop-baseline.log, polling-loop*.txt |

Six reproduced failing tests do not establish six independent root causes. SQL slow-acquisition
warnings appear in the all-long log, but neither a connection leak nor its cause has
been established. Both primary and tool polling schedules can fail; the first primary
pass was not stable evidence. Its fixture rebuild is recorded in `build-primary-loop.log`.
Bug-finder labeled the intermittent baseline TEST_CONFLICT; the execution evidence is
accepted, but there is no conflicting assertion or test defect: baseline reproduction
is the purpose of M1. Keep the test strict and resolve production behavior in M3/M4.
Eight existing controls passed; the intermittent primary test is not counted as one.

Retained controls not yet rerun in M1: `api::p3_promise_suspend_survives_executor_restart`
(host-api-tests), `rpc::raw_sync_rpc_resumes_after_suspension` and
`rpc::raw_async_rpc_resumes_after_suspension`, `rpc::rpc_suspension_retries_after_concurrent_http_wait_finishes`,
`rpc::durable_streaming_output_recovers_after_executor_restart`,
`rpc::durable_streaming_input_recovers_after_executor_restart`, and
`wasi::interrupt_while_parked_in_p3_sleep` (host-api-tests). All listed RPC controls use
the `agent_rpc_rust` fixture in test-components/agent-rpc; rebuild before execution.
These are not new green claims; A2/A4/A6/A7 coverage
and the expanded owner/stream/race acceptance cases remain M3–M5 obligations.

### Bounded implementation contract

- `worker/instance.rs::OwnerExecution`: shared eligibility state only. Register owner
  work at existing admission boundaries, including native/entity work before Store creation.
- `workerctx/default.rs`: propagate owner/context registration;
  `InstanceHost::create_store`: install runtime observations. Do not replace Store ownership.
- `worker/invocation.rs::run_guest_call_settled` and plain `run_concurrent` streaming
  paths: observe runtime facts, including post-root tasks. Root return is not idleness.
- `durable_host/{suspendable_wait.rs,io/poll.rs,p3/clocks.rs,golem/v1x.rs,wasm_rpc/mod.rs}`:
  replace local elections with one owner decision; aggregate deadlines/activation;
  revalidate the same evidence after wakeup persistence.
- `durable_host/durable_session/mod.rs`: distinguish passive source waits from active
  journaling/delivery using existing ownership. Unknown work denies eligibility.
- Keep existing timestamped Suspend, set_interrupting, retirement/interrupt precedence,
  last_resume_request checks and discard/replay. Quota/fuel/explicit interruption are
  not subject to the automatic-idleness predicate.

### Named feasibility gates (specified, not yet executed on the new implementation)

**P2 production dispatch (M4/A3):** use the narrow owned-timer dispatcher through actual
SDK production bindings. Pass requires actual long-sleep unload/resume, both independent
HTTP/P2 schedules progressing with exact result, one effect and follow-up, plus preserved
poll duplicates/resource lifetime/readiness. Production borrow, progress, readiness or
replay violation is FAIL. Synthetic-only success or unexercised overlap is INCONCLUSIVE.
Block/ready/read/write coverage remains separate A3 acceptance, not implied by poll(list).

**Stream discard/replay (M4/A6):** exercise source waiting versus active journaling/delivery,
post-result activity, non-flat values/nested handles and supported directions. Pass requires
actual eligible unload/reconstruction, no suspension during runnable work, exact offsets,
values/terminals/effects. Loss, duplication, replay violation or invented EOF/Cancelled is
FAIL. No unload, scalar-only evidence or unexercised boundary is INCONCLUSIVE. Either
non-pass pauses for a bounded scope decision; it does not authorize a cleanup barrier.
