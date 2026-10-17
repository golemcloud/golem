# PR #3894 delivery plan

## Target

Deliver monthly resource policy, accounting, admission and enforcement on current main's recovery
model. Do not spend another cycle repairing the P2 HTTP ownership chain or suffix-rollback
machinery that the recovery work removes. Do not build a second recovery or worker lifecycle model.

This plan replaces the earlier incremental control-removal queue. `PR-3894-feedback.md` remains the
source of feedback status. Planning does not close a review finding or authorize implementation,
rebasing, pushing, review replies or issue-status changes.

## Merged baseline and ownership

The full [recovery report](https://t-03h1xykjf7qcai8l0jyytjz1f-p21914.onamp.dev/#full-0-summary)
was read, including sections 0-9 and every exploration in 8.1-8.6. The user confirmed that its
conclusions are being implemented now. The dependency map below includes the merged changes
reported by the user and verified through GitHub on 2026-10-16:

| Work | Owner and delivery boundary |
| --- | --- |
| [#4199](https://github.com/golemcloud/golem/pull/4199), Rust P3 HTTP templates/guidance | Daniel Vigovszky; merged into main at 06:50 UTC |
| [#4198 / GOL-910](https://github.com/golemcloud/golem/pull/4198), P3 HTTP repair | Daniel Vigovszky; merged into main at 12:34 UTC. Reuse its call-local send and verified body continuation; do not recreate it. |
| [#4212 / GOL-898, GOL-897](https://github.com/golemcloud/golem/pull/4212), client tool stdin terminals | Daniel Vigovszky; merged into main at 12:03 UTC. Preserve its cancellation semantics, native-tool propagation, tests and Python artifact pin. |
| [#4207 / GOL-920](https://github.com/golemcloud/golem/pull/4207), call-local recovery and removals | Daniel Vigovszky; merged into main at 22:46 UTC. Its runtime, SDK, WIT/schema and fixture contracts are now part of the baseline. |
| PR #3894 / GOL-442 | This workstream; owns monthly limits and the smallest necessary integration with the surviving lifecycle |

#4207 removes P2 HTTP, the global idempotence mode, atomic regions, guest cursor/checkpoint controls
and suffix-rollback scans. It keeps verified P3 continuation and host-side HTTP retry, changes RDBMS
recovery to typed loss where resources cannot resume, and preserves operator fork/revert. It does
not add a generic guest retry schedule, a new socket API or recovery-decision instrumentation.

The user has authorized local integration and now directs this work to current main. #4207 is
merged; it is no longer a separate branch dependency or handoff gate. The previously validated
integration is checkpointed locally before updating against fetched main. Its exact revision and
source receipts remain validation history, not the delivery target. Main includes #4198, #4199
and #4212, including the stdin producer-cancel observer and consumer-cancel Dropped terminal.
Preserve the ordinary pre-finalization terminal check and Always/Deferred commit-before-notify.

#4198 supplies P3 retry and continuation; #4207 supplies the removals, now in main. Older ticket
wording about post-1.6 work is not a reason to repair the discarded model. Integration remains
local; pushing, review replies, tracker changes
and grant-precedence changes are not authorized.

### Newly merged stdin overlap

#4212 directly overlaps this branch's durable-input producer, tool stdin pump, durable-session
tests, tool-streaming tests and provider fixture. Keep unsolicited consumer cancellation as
`StreamResult::Dropped`; Wasmtime permits `Cancelled` only for a requested finish. Producer
cancellation still ends agent-method input normally. The real production reason observer lets
client-tool stdin report `ByteStreamFailure::Cancelled`, with the reason read from its journal.
It is not the deleted test-only receive observer and must not be removed as F11 cleanup.

Preserve the Python tool's KeyboardInterrupt/exit-130 behavior and its upstream artifact pin.
Do not duplicate GOL-898/GOL-897 fixes or rebuild/publish that artifact here. Monthly typed
interruption, entered-effect ownership, consumer-journal progress and raw-prefix guarantees still
need composition checks after integration. Earlier local receipts prove their original branch
trees, not the newly combined runtime, SDKs or fixtures.

## Starting state

This section records the state at plan capture, not the status of every later execution step.

- The earlier executor checkpoint is pushed. Later CLI, window-close and failure-origin changes
  have local validation receipts and local checkpoints. They remain unpushed.
- The current success-boundary migration is unfinished. Production compilation passed, but its
  test build stopped at an old `selected_writer` helper call in `worker/stop.rs`. No tests ran in
  that final batch. Do not describe the working tree as fully validated.
- All owned build/test jobs are stopped. Implementation stays paused during this planning task.
- Preserve both the validated work and the unfinished tail. No reset, silent rollback, cache
  deletion or restoration of old fixture files to match historical hashes.

## Authorized work and integration boundary

#4195 merged after the previously integrated main revision. Local integration of fetched
main, including #4195/#4220/#4223/#4224, now passes its selected combined-source checks. The impact
review and verification scope are in `PR-3894-pr4195-impact.md`. They cover automatic suspension and
quota-forced Suspend during custom bodies and pending async finish. The upstream "suspended sibling"
case uses simulated crash, not Worker auto-suspension. Keep SDK/WIT/host integration coherent and
preserve this branch's InterruptSources behavior and #4220's owned Start/claim and declared
host-internal-read contracts. The observer removal has since passed its guarded small-batch checks
and is checkpointed. Cooling and the thermal/CI-time watchdog are in place. Fresh binaries and rebuilt
fixtures pass the scoped native checks, including both custom-finish completion orders. A
reverse-order baseline exposed and corrected an ambiguous test gate through the existing oplog
fixture, without a runtime hook or weaker assertion. Library checks, strict executor/test-utility lint,
production compilation and browser checks pass separately. Earlier receipts remain tied to their
own source trees. A1/F10/F11 and the owned-Start guest-drop/quota conjunction remain open.

The user approved independent implementation, then local recovery integration, and now directs
the branch to current main after #4207 merged. Keep #4212's merged stdin fixes.
This does not authorize changing grants, pushing, review replies or tracker mutations.

The earlier local integration is checkpointed. Its selected combined library,
native socket/HTTP, lifecycle, metering and CLI scopes pass, and current fixtures rebuild and
validate. Production compilation without `test-utils`, strict executor/CLI lint and account checks
pass. The current-main update has resolved conflicts and passes the selected library, native,
lifecycle, metering and CLI/client checks plus strict all-target lint and production binary checks.
An existing privileged-XFS case was not executed in this shell and remains
outside that evidence. The user has authorized skipping the obsolete hooks for local
commits in this task. Use command-scoped bypasses; shared Git configuration is unchanged.
No remote publication has occurred.
The canonical wallet installer owns the one replay-generation reset. Startup provenance follows
successful installation, not the earlier attempt counter. The local restoration now uses main's
establishment owner and removes stop publication, progress and admission seals. Combined library,
native host/lifecycle/accounting, real entity-blob ownership/repair, strict lint, production and browser
checks pass. A1 remains open for the broader retained-change review; F10/F11 controls and the native
filesystem proof/profile gaps remain separate work.

Monthly monitor failures and operational policy outages now belong to the existing window target
as failure data, not synthetic completed stop futures. Native health assertions remain unchanged;
focused and consolidated checks, strict lint and the production check pass. This separates the
monthly health obligation before removing the remaining generic driver registry and seals. It does
not close A1 or the remaining F10/F11 requests.

- Complete the surviving success-boundary simplification as one coherent candidate, not as a new
  recovery repair. The current #4207 implementation retains the same ordinary boundary and
  commit-before-notify contract. Its establishment owner remains the later integration target.
- Keep the validated independent policy/accounting/CLI changes ready for delivery.
- Classify the finite remaining A1/F10/F11 work by surviving contract and actual #4207 overlap.
  Delete or simplify only independent PR-introduced machinery; do not rebuild removed recovery.
- Integrate current main and its recovery, SDK/schema and fixture contracts locally.
  Preserve protocol-owned repair and typed loss rather than restoring removed recovery behavior.

## Scope decisions

### Fixed closure queue

The MVP acceptance bar is safe eventual stopping, not a perfect executor or an instantaneous hard
cap. The applicable exhausted monthly cap must reach the existing Worker stop path. Extra detection
or safe-unwind latency, including another normal monitor interval, is acceptable. Committed history,
invocation identity, entered effects, settlement and ordinary reconstruction must remain correct.
Ephemeral work keeps its resource-specific terminal outcome rather than being replayed. Keep actual
usage through window closure; delay is not permission to discard work or refund it prematurely.

Exact stop-time bounds, quota winning every race with a short guest timeout, and exhaustive rare-
interleaving proofs are not MVP delivery gates. If success or a guest timeout wins naturally, retain
its actual result instead of forcing a quota outcome. Use sensible timeouts to catch hangs, not as
new product latency guarantees. Relax only timing/order expectations under this decision, not the
durability, accounting, effect, identity, terminal or cleanup assertions. Broader review cleanup must
stay explicitly reported; this acceptance bar does not silently mark unresolved F10/F11 requests done.

This is a disposition queue, not a requirement to build a new test or infrastructure layer for every
listed uncertainty. Only a concrete current production safety/durability issue blocks the functional
MVP. Reuse the existing Worker interruption/reconstruction guarantees and already-valid evidence.
Unverified combinations and unexecuted backend profiles stay visible limitations, not invented bugs
or silently completed review requests.

The remaining work is limited to this queue. Do not refill it with optional cleanup or build a new
framework to preserve discarded private-state projections. Keep the reviewed integration baseline
and reuse source-bound validation for unchanged code. Batch related changes rather than treating
each native helper as a separate refactor and verification project.

New edge-case work must first show a reachable path in the current production code, using supported
inputs, APIs or realistic operating failures. A hypothetical future caller, changed production code,
impossible private state or invented ordering guarantee is not a delivery blocker. A fixture may
observe the current path or reproduce a delay/error that the real dependency can produce; it must
not manufacture a new contract. Existing fast regressions need not be removed or weakened, but do
not expand them into another fault-injection or infrastructure project. Keep explicit review cleanup
separate from a diagnosed correctness blocker, and report unverified claims as such.

| Work item | Required disposition and exit |
| --- | --- |
| A1 retained runtime delta | Resolve the concrete internal `Worker::resume_replay` scope finding: monthly admission is required, but changing enqueue acknowledgement into replay-completion acknowledgement is not demonstrated as required. Retain the reviewed interruption, window, startup, health and unload owners. Remove the unrelated reply protocol or document a concrete monthly dependency. No new general lifecycle mechanism. |
| F10/F11 controls | Record an explicit bounded disposition for the remaining monthly-clock and keyed-filesystem-gate families. Do not make a perfect historical test-control purge a functional MVP gate without a current production/durability dependency. Preserve business assertions; retire removed-producer projections and unnecessary precise timing/order guarantees under the acceptance bar above. A pre-wait observation is not timer-ready or deadline evidence. If removing a family, validate consumers before deleting its complete producer and validate that deletion separately. Report any remaining review cleanup honestly. |
| Retained test support | Keep the actor-after-published-success and deleting-first fence only under their documented minimal-scope justifications. Bound scripted allocation, entity construction and read-only projections to accounting/entity/ownership evidence. Do not call them native-XFS, syscall Pending, dispatcher coverage or reviewer acceptance. No further abstraction, renaming or helper extraction is required by the completed standards review. |
| Owned-Start composition | Exercise actual guest Drop and a later real guest checkpoint while the entered Start remains held, then apply monthly interruption. Require accepted-work ownership, exact original history/terminals, settlement, physical release and ordinary reconstruction. The existing owned-start interrupt race alone does not close this gap. Diagnose an actual failure before any runtime fix. |
| Filesystem evidence and profile | Replace the misleading pre-poll keyed gate without relabeling withheld delivery as native metadata-future Pending. Run the exact required managed-XFS profile cases, or report the observed environment blocker precisely. Do not start a new filesystem/backend project or claim unexecuted profile coverage. |
| Final delivery validation | After the required changes form one coherent candidate, run the affected native/body checks and final lint/production/format/documentation checks for that source. Preserve failed receipts and explicit remaining limitations. Write the PR explanation in simple technical English, without verification transcripts, test counts or commit hashes. No push, review/tracker mutation or grant-precedence change. |

The bounded standards review found no new documented-standard breach. The scope review found one
additional acknowledgement-contract justification item, not a new diagnosed runtime bug. These
reviews support the queue; neither is a blanket A1 completion or a full-repository pass. Findings
outside this queue require a demonstrated monthly dependency before they become delivery work.

### Remaining integration queue

| Group | Next action |
| --- | --- |
| Surviving success boundary | Locally validated with unchanged native assertions, consolidated affected checks and strict lint. Carry it forward without claiming the remaining A1 protocol is resolved. |
| Generic stop/writer/generation machinery and its teardown/idle/private-state controls | Private result writer, StopPublication, StopAdmissionSeal, stop-task registry and early global handoff removed and validated. The complete teardown-fence and completed-history replay-gate families also pass separate consumer and post-removal checks. Main's establishment and canonical wallet installer own their work. Broader retained-change review and idle/tail/monthly-time/deletion/private-control dispositions remain open. |
| Filesystem observation gate | Remove the keyed semaphore gate in `wasi_filesystem/mod.rs` and its producer/consumer wiring. Its Pending event precedes polling the actual filesystem call; it is not native metadata-Pending evidence. Use actual wait/adapter proofs and separate Worker guarantees. This is retained filesystem support, not removed P2 HTTP. |
| UDP/RPC/WebSocket test observation | Complete observer families and WebSocket reader-order gates removed. Existing tracing and an own-agent guest promise preserve native assertions without Worker test state or a recovery redesign. |
| Monthly fault/time, replay, deletion and allocation support | Monitor panic selector, invocation-deadline, settlement-loss and actor stopper/monitor-exit controls removed and validated. Real append failure covers later-idle and pending-TCP actor loss. The initial-idle manual actor-stop cell retires, while real monitor-fault startup coverage remains. Idle, monthly-time, replay, deletion and allocation controls remain open. |
| P2 HTTP, suffix rollback, atomic/mode-off and changed resource recovery | No new repair work here. Use main's recovery contracts and retire removed contracts rather than rebuilding their old tests. |

The queue is not a requirement for one build or one new test framework per row. Combine related
changes into coherent candidates and reuse source-bound evidence for unchanged code.

| Area | Delivery treatment |
| --- | --- |
| Registry policy, consent, grants metadata/expiry, API and CLI | Keep the completed work. Grant replacement precedence stays unchanged until G1 is discussed. |
| Usage delivery, blob rollover, flusher deduplication, backend capability checks and live-only billing | Keep the fixes and exact financial/safety guarantees. Revalidate changed interfaces on the chosen recovery revision. |
| Entered blob/DB/KV work and unknown outcomes | Keep effect ownership and quota settlement. Call-local repair does not make uncertain external effects exactly-once or justify an early refund. |
| Monthly detector, stale-window/capacity validation and admission | Keep. Integrate with surviving typed interruption, invocation exit, unload and reconstruction. |
| Extra generic stop/result/generation protocol in this PR | Removed in the validated local restoration. A1's broader retained-change review remains open; preserve ordinary responsibilities rather than restore whole old-main files. |
| P2 HTTP sessions, body-repair chains and `http/inline_retry.rs` | No further repair or replacement test infrastructure here. #4207 removes these paths. Retire their PR-specific controls with the removed producers. |
| Atomic regions, suffix rewind, mode-off recovery and guest runtime Jump | No further repair here. Do not restore deleted interfaces or build new ordering guarantees for them. |
| P3 HTTP recovery, stable request keys and verified continuation | Use #4207's implementation and its owner's evidence. Do not duplicate GOL-910/913/915 or the old POST/WebSocket investigation. |
| P2 IO/filesystem/socket support, durable streams, RPC and WebSocket paths that survive | Keep required monthly interruption coverage. P2 HTTP removal is not removal of all P2 waits. Do not redesign surviving replay policy as part of quota work. |
| RDBMS | Use the new typed-loss contract. Preserve completed history, transaction-marker ownership and settlement probes. Do not restore automatic transaction suffix replay to satisfy an old test. |
| Test controls and Worker-owned scenarios | Delete obsolete producers, state, setters and consumers together. Replace or justify controls only for surviving guarantees. F10/F11 remain open until that is done. |
| Diagnostics | Keep ordinary tracing and existing OTLP integration. No new exporter, recovery-decision oplog feature or replacement diagnostics project. |

New typed-loss behavior must have exact tests for that contract. An old transparent-resume test
can be retired only when its owning contract is actually removed from the selected integration
revision and the retained durability/effect guarantees are covered. Never make a failing test accept
either old or new behavior, skip replay errors, or label an unresolved failure "superseded".

## Delivery sequence

### 1. Fix the integration boundary before more repairs

- Use fetched main, which now contains #4207. Keep the actual fetched revision in validation
  receipts so results stay source-bound; there is no separate unmerged dependency.
- Main also contains merged #4198, #4199 and #4212. Preserve their changes through
  conflict resolution, including #4212's distinct consumer and producer cancellation branches.
- Use a file-and-contract overlap map. Current overlap includes Worker/loop/context code, durable
  host/streams/RPC code, test-utils, shared documentation, fixtures and generated schemas.
- Account for renames: #4207 edits `worker/instance.rs`; this PR moved it to `worker/instance/mod.rs`.
  Port its surviving content into the directory layout rather than recreate a competing flat module.
- Keep independent Registry/CLI/accounting work ready while the overlapping executor work settles.
- Local integration of main is authorized. Account for each newly fetched delta before reusing
  earlier evidence. Upstream validation remains distinct from monthly-composition evidence.

Exit: one concrete recovery contract and revision for executor work, with ownership of each overlap.
No compatibility layer for both lifecycle models and no speculative port to several moving heads.

### 2. Produce one coherent monthly-integration candidate

- Carry forward the validated independent repairs and CLI organization.
- Reassess the unfinished success-boundary tail on that revision. Complete it only if it is still
  needed for A1. Otherwise preserve it outside the delivery change with an explicit disposition.
  Do not make its lone old helper call the start of another old-baseline repair cycle.
- Remove this PR's unnecessary `StopPublication`, `StopAdmissionSeal`, selected-outcome/writer
  coordination and early global generation handoff rather than merely rename or relocate them.
- Use the surviving interruption/unload owners. Scope any extra scheduling to monthly detection;
  do not move general lifecycle ownership to make quota ordering exact.
- Preserve current-owner/window/capacity revalidation and fail-closed policy availability. A policy
  outage must not permanently poison the cached Worker, and real cleanup failure must stay visible.
- Preserve concurrent successful completion and bounded detection/unwind overshoot. Settle the
  execution window before permit release; physical filesystem reservation/repair remains independent.
- Rebuild affected fixtures against the selected SDK/runtime interfaces before native validation.
  Regenerate shared schema outputs from the combined canonical sources; do not restore deleted
  variants or old Wasm artifacts to satisfy historical hashes or old expectations.
- Delete test-control families whose production path or discarded protocol disappears in this
  candidate. Handle the remaining F10/F11 families as a finite list, not a new discovery loop.
- Retire tests that specify only the discarded stop/writer protocol with that protocol. Preserve
  their real result/history/cleanup guarantees through the surviving production paths, rather than
  invent another protocol to keep obsolete private-state assertions passing.

Exit: normal and test builds compile; the executor diff contains monthly integration and demonstrated
dependencies, not a second general lifecycle protocol. Every retained broader fix has a concrete
dependency or is kept out of this delivery. No claim that A1 is satisfied just by retaining Worker.

### 3. Validate the surviving guarantees, then the complete candidate

Start with the focused dependency/behavior checks, then run one consolidated affected cohort on
the coherent candidate. Do not rebuild the full executor after each removed helper.

- Policy recovery on the same cached Worker, disabled meters, hard caps and applied overage mode.
- Detection during real pending host work, stale proposal rejection, admission and entity memory.
- Entered effects and uncertain outcomes, exact usage/remainders/month rollover, and no billing of
  materialization or completed replay. No accrual after the execution window closes.
- Durable result commit before notification, exact invocation identity and provider-side effects,
  committed raw-prefix equality, single terminals, physical cleanup and ordinary recovery.
- New recovery contracts: preserve sibling history and operation keys; report the exact typed loss
  when a resource cannot resume. Do not require a new success or transparent replay of a removed API.
- Causal tests at real dependency or production-body boundaries. No sleeps/retries for ordering,
  reduced concurrency, larger guards, fabricated projections or copied lifecycle models.
- Scoped strict lint, normal production build without `test-utils`, affected API/CLI/schema checks,
  and one aligned browser/documentation pass on the final source.

Reuse #4207's recovery coverage rather than rebuild its whole migration here. Its remaining real-DB
crash-window and injected HTTP payload-outage validation belong to that owner. This PR's monthly
composition evidence must not be presented as a substitute for those scenarios.

Build only in the primary checkout with incremental compilation disabled. Use measured capacity
and default compile/test concurrency unless evidence justifies a limit. Bind receipts to the tested
source; old receipts are useful history, not proof for the newly integrated tree.

Exit: each relevant finding has a locally demonstrated disposition. Classify any failure as a
monthly dependency, existing defect, intermediate regression or retired-contract assertion before
choosing a repair. No open engineering defect hidden behind a green test selection.

### 4. Reach the grant-precedence discussion

Before returning G1 to the user:

- A1 is locally satisfied and F10/F11 have complete, concrete dispositions on the surviving model.
- Completed F1-F9/C1/F12-F14 behavior remains intact after integration.
- No old-recovery repair, unrelated WebSocket/Git investigation or new upstream feature is being
  treated as a monthly-limit prerequisite.
- Remaining upstream handoff/validation dependencies are explicit. If one blocks executor delivery,
  report that dependency rather than repair the discarded model while waiting.
- Prepare a short grant decision brief covering plan upgrades, lower/higher grants, expiry and
  owner overrides. Make no precedence change yet.

Target: G1 is the only remaining product-policy decision. A dependency's actual availability is
not grounds for promising an unsupported date or calling the PR complete.

### 5. Apply the decision and publish the delivery

After the user chooses G1, implement only that choice and validate its affected resolution/expiry/
API/CLI behavior. Then reconcile the PR body and local response drafts with the final checklist.
Use simple technical English describing changes and reasons, not test counts, command transcripts
or commit hashes. Preserve the agreed Registry-delivery and chronological-attribution MVP limits.

Create reviewable checkpoints at coherent validated boundaries, using normal hooks. The previous
one-command hook exception is consumed. Obtain any required publication authorization; do not infer
permission to bypass hooks, push, post replies, resolve threads or change GOL-442 status from this
planning request. No overall acceptance claim before the corresponding review decision.

## Control against another open-ended repair cycle

Every proposed change must answer: does it survive #4207, is it required for monthly enforcement,
and what concrete guarantee demonstrates that dependency? If the path is removed, do not replace
its test hook or fix its old recovery behavior. If it survives but is an independent defect, keep
it outside this delivery unless a reproduced monthly dependency makes it unavoidable.

The next implementation action is the integration/overlap gate, not resuming the old helper queue.
