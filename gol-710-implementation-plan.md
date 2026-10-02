# GOL-710 implementation plan

2026-10-02. **Implementation plan — steps 5–6 reopened at design review.** The original final review
returned “APPROVE — no blockers to implementation” and required no further
pre-implementation experiment. Approval covers this handoff, not correctness of
an unimplemented fix or permission to ship partial work. The step-5 integration
review subsequently identified an unresolved Store-scheduling dependency; see
section 5 below. Do not implement an all-scope publication barrier as currently
written without resolving that dependency.

Implementation is in progress locally. Steps 1–4 have Oracle and bug-finder
approval. Step 5 has been investigated but has no implementation or approval.
Steps 5–8 remain outstanding. No changes have been committed, pushed, or deployed.
The original final plan was uploaded to GOL-710 before implementation began.

This is the implementation handoff. It supersedes the open alternatives and
conditional implementation sequence in
[the investigation record](gol-710-status-reconstruction-design.md), which
retains the historical rationale and caller inventory. Read repository and
scoped guidance and load `understanding-durable-execution` and `testing` before
implementation. Use `modifying-test-components` for the targeted guest fixture.

## 1. Evidence and selected fix

Baseline: [accae0e43](https://github.com/golemcloud/golem/commit/accae0e435b5097cd1d7940a5c1f568bde8a055a).
Re-resolve symbols if implementing on a newer revision; do not assume current
`origin/main` is this baseline.

The [child experiment](https://ampcode.com/threads/T-01a0fbc3-a952-7195-bfaa-3509e483aa14)
ran actual guest P3 HTTP/clock accessors, Worker oplog commits, and a second
executor reconstruction. Gates selected these schedules deterministically:

| Schedule | Result on baseline | Decisive history |
| --- | --- | --- |
| Cleanup before sibling claim | Pass | Jump 30 skips 12–30; clock gets new Start 31, End 32→31, Delivered 33→31. |
| Sibling claims before cleanup | Fail: second reconstruction times out | Clock claims Start 14; Jump 30 skips it; End 32→14 and Delivered 33→14 survive afterward. |
| Sibling issued before scope recovery | Fail: second reconstruction times out | Same invalid surviving records; moving only Live publication cannot fix an already-issued handle. |
| Ordinary retained-Start adoption, no Jump | Pass | Clock retains Start 12, with End 28→12 and Delivered 29→12. |

All first recoveries logged success. All second runs kept HTTP effect count at
2→2. Failing replay explicitly skipped the orphan terminal and delivery marker.
The completion-token error occurred after timeout during teardown, not as a
proven initiating failure. The fixture establishes incomplete history by logical
revert followed by executor teardown/recreation; it does not reproduce the
original process-crash/status-assertion abort. The original incident's exact
schedule remains unproved; no further pre-implementation experiment is required
to establish the two defects addressed here.

Evidence is retained in the child orb. Local copies are
`tmp/gol-710-real-path.patch`, `tmp/gol-710-real-path-evidence.txt`, and
`tmp/gol-710-real-{claim,issued,cleanup,control}.log`. These are investigation
artifacts, not production changes. The patch includes test instrumentation and
must be reviewed and hardened, not applied blindly as a finished regression suite.

**Implement two separate corrections:**

1. The status actor owns projection repair through a fixed acknowledged oplog
   boundary. Callers never select or assert attachment. Early commit responses
   remain independent of projection completion.
2. Shared replay-transition state owns destructive recovery. When a selected
   Jump invalidates an issued replay call, discard the resident reconstruction
   using the existing `InterruptKind::Jump → RetryDecision::Immediate` path.
   Do not re-admit that call handle in place or charge an application retry.

The second correction may add a Store reconstruction on exceptional Jump
recovery. Accept that cost instead of a second in-place call-recovery protocol.
Status repair does not authorize the old runtime to continue.

## 2. Contracts that every step must preserve

- Oplog history remains authoritative. Detachment is a reconstructible derived
  state, never a reason for the status assertion or stale admission.
- Authoritative reads are actor-ordered and non-committing. They observe completed
  projection work, not arbitrary later buffered entries.
- Durable and ephemeral commit semantics stay distinct. Ephemeral `Deferred`
  acknowledges writer handoff; `DurableOnly` does not flush that buffer.
- Genuine authoritative-storage errors follow existing backend retry/fail-stop
  policy. Optional checkpoint failure is an optimization miss. Fencing retires
  the old owner. Do not invent a projection-specific outage recovery policy.
- Ordinary replay with retained Starts remains supported. Only history-changing
  recovery introduces the new transition obligation.
- A terminal or delivery may be recorded under an existing Start only while the
  resident execution still owns a valid claim to that settled history.
- An accepted recovery operation outlives its initiating waiter. Its storage
  work cannot depend on polling the Store it may need to replace.
- No cursor, advance-gate, card-boundary, or lifecycle lock may be held across
  storage/projection waits or drainage of the affected Store/entities.
- No protocol/WIT changes, backward-compatibility path, HTTP idempotence changes,
  weaker replay validation, or generic lease/capability framework are required.

## 3. Step-by-step implementation

### Step 1 — Preserve and strengthen the real-path reproducer

Start from the child patch. Owning files:
`golem-worker-executor/tests/http/transition_probe.rs`, `tests/http.rs`,
`golem-worker-executor-test-utils/src/lib.rs`, the existing
`workerctx::ReplayAdmissionHook`, and
`test-components/http-tests/src/http_client_4.rs`.

- Keep deterministic gates before destructive cleanup and around sibling
  admission/issued handles. Keep them outside the locks and actor being tested.
- Remove the diagnostic assertion that primary Live precedes Jump: it describes
  the broken ordering, not a contract the fixed regression should preserve.
- Replace numeric schedule modes and ad hoc prints with the smallest readable
  local test structure. Preserve all four schedules.
- Explicitly assert both nested results of the first recovery's `join!`; the
  diagnostic logs both but only propagates the coordination result.
- Assert actual Start/terminal/delivery references, returned results, expected
  first-recovery effect counts, and no new effect on the second reconstruction.
- Build only the HTTP fixture before execution. Keep intentional migrations
  produced by the current CLI build, as required by repository guidance.

**Exit:** observe the two expected baseline failures and two passing controls.
Save individual command exit statuses; the diagnostic shell loop itself exited
zero even though two cargo invocations failed. Do not reinterpret that as green.

### Step 2 — Add a bounded projection reader and exact transaction horizon

Owners: `worker/state_actor.rs`, `worker/status/mod.rs`, and existing opened-oplog
read interfaces. Preserve current cold persisted reconstruction callers.

- Fix the projection horizon from acknowledged coverage: the successful
  buffer-flushing commit's pre-commit sample and receipt/watermark evidence,
  including threshold/direct-commit receipt gaps. Never use a later raw buffer
  tip as proof of acknowledgment. For ephemeral `DurableOnly`, do not advance
  coverage from the pre-commit buffered tip.
- Bound region discovery, baseline validation, candidate retries, and folding to
  that same horizon. Factor existing fold logic rather than duplicating it.
- For resident ephemeral reconstruction, use opened-oplog bounded reads that
  include handed-off entries awaiting persistence. Exclude newer buffered work.
  Do not add an `Always` commit merely to make reconstruction readable.
- Preserve external payload hydration, field-specific region semantics, and
  region-aware fallback when a later revert removes an offending record.

**Exit tests:** blocked ephemeral writer after handoff; newer buffered sentinel;
receipt gap; durable commit; ephemeral `DurableOnly`. Assert exact resulting
status and horizon, no missing acknowledged suffix, no sentinel inclusion, and
no reconstruction-induced commit. Attached reads add no storage access.

### Step 3 — Make projection maintenance an actor obligation

Owners: `StatusState` commit/fold/publication, `status_flusher`, and lifecycle
notification integration.

- Keep the early commit receipt at its current persistence milestone.
- After that receipt, the same actor-owned transaction completes incremental
  folding or reconstruction before dependent jobs run, even if the producer
  cancels or replay registration fails.
- Use one publication path for incremental and reconstructed status: metrics,
  recovery index, authority generation, cache reconciliation/flush policy,
  notifications, and appropriate archival scheduling.
- Never continue an authoritative operation with the retained invalid record.
  Remove the ignored repair-failure path in `AppendInvocationIfVersion`.
- Preserve existing storage failure/fencing policy. Distinguish baseline
  rejection from authoritative read failure. Do not test successful waiter
  recovery after deliberately process-fatal storage failure.
- Keep hydrated invocation-result cache resets at their explicit runtime
  boundaries, separate from routine projection publication.

**Exit tests:** cancel producer after accepted Jump commit; poison replay
registration; multiple Jumps with committed prefix; invalidation between the
two failure-handler status reads. Gate projection processing and prove an early
successful response completes while later authoritative work waits.

### Step 4 — Remove caller-visible attachment APIs

Owners and consumers: all eleven asserted reads and the attached/raw reader
inventory in investigation section 9, including test-utils and gRPC consumers.

- Delete `NonDetachedStatus` and the asserted worker getter; no aliases or
  replacement `try_non_detached_status`.
- Provide one non-committing authoritative operation with the existing lifecycle
  failure semantics. Detachment is not an exposed error or caller choice.
- Keep read/decide/append admission atomic inside the actor. Other consumers can
  use immutable actor-ordered projections without moving all decisions there.
- Separate existing explicit reattachment call sites into their actual purposes:
  persistence barrier, runtime cache reset, or now-redundant projection repair.
- Classify raw readers individually. Do not add a status wait to early response
  publication or make actor-internal oplog/plugin callbacks await the same actor.

**Exit:** removed symbols/assertion absent; affected consumers compile; metadata
retirement fallback, admission, authority synchronization, result lookup, stream
history, and success/failure notification tests preserve their contracts.

### Step 5 — Give destructive replay recovery one shared owner

Owners: `durable_host/replay_state/{mod,cursor,claims}.rs`, transition machinery
in `durable_host/mod.rs`, and incomplete-scope paths in `concurrent/call.rs`.

- Register unresolved rollback-capable scope obligations at replay discovery /
  admission, including retained scopes whose host owners have not completed
  classification. Reuse existing scans; add no happy-path storage round trip.
  This prevents a competing settler from repairing a sibling into the live tail
  before a late scope owner selects a destructive Jump.
- Classify the recovery against a fixed replay target; reserve its selected
  regions before releasing incomplete handles or permitting live repair.
- Store operation identity, target/generation, regions, and outcome in shared
  transition state. Use a worker-owned independent continuation for append,
  commit, and settlement; do not put it in the status actor or initiating
  Store-owned future.
- Competing primary settlers, non-primary continuations, missing-Start recovery,
  and incomplete repair wait on the same obligation and re-evaluate afterward.
  Old pending transition tokens cannot publish against changed targets.
- Record issued Start indices as claims are accepted, before handles escape.
  Retain the reconstruction-scoped information through settlement: resolver
  membership alone misses consumed resolutions and transferred delivery tokens.
  Release it at a proven reconstruction boundary; do not retain a worker-lifetime
  history of claims.

**Settlement choice is fixed:** if no issued claim is newly invalidated, register
and prune the Jump, finish bookkeeping, then permit continuation. If an issued
claim is newly invalidated, freeze that resident reconstruction and take step 6.
Do not choose an in-place fresh-Start replacement for the old handle.

**Exit tests:** competing settlers, target growth, late scope classification,
unaffected retained Starts, and cancellation before/after acceptance. An accepted
operation completes independently; no stale Live publication or lost wakeup.

### Step 6 — Rebuild invalidated resident execution without recording new damage

Owners: the shared transition state, Worker interruption queue/fence/notify
mechanics, existing `InterruptKind::Jump` handling and invocation-loop teardown,
`concurrent/{call,access,delivery}.rs`, and entity-body lifecycle integration.

- Once rebuilding is selected, freeze new effects, old-Start repair, terminal /
  delivery / cancellation recording, and invocation-success publication from
  that resident reconstruction. The independently owned Jump operation continues.
- Serialize this fence with append reservation/enqueue, including synchronous
  completion-marker recording. An earlier async validity check is insufficient.
- After required Jump commitment, wake/interrupt the primary and fence entity
  bodies using existing Jump semantics. Do not wait for Store/entity drainage
  from the originating accessor or under cursor/card locks.
- Use `Jump → Immediate` to tear down and reconstruct, with no invocation Error,
  application retry-budget charge, or fabricated guest cancellation. Preserve
  arbitration with real failures, explicit interrupts, deletion, and shard loss.
- Make intentional reconstruction abandonment visible to session Drop,
  `AccessTerminalGuard`, live/replay completion tokens, and dropped-call drains.
  Activate it before the failure handler drains dropped calls; that drain
  currently precedes the handler's special-case Jump return.
  Do not synthesize `Cancelled`, `CompletionDiscarded`, or token-dropped
  divergence for the abandoned execution. Still release permits/memberships and
  settle accepted append tasks. Do not erase genuine operations accepted before
  the boundary by clearing the whole cleanup queue.
- Join the old transition continuation before replacement execution starts so
  it cannot append a late Jump into the replacement runtime.
- Restart only for newly invalidated issued claims, not merely because a cleanup
  Jump exists. The replacement reads corrected history; test that redundant
  cleanup cannot trigger an unbounded rebuild loop.

**Exit:** all four real-path schedules pass both reconstructions; no newly
surviving terminal/delivery refers to a skipped Start. Also test resolutions
already obtained, transferred completion tokens, repair before scope selection,
caller loss after acceptance, partial multi-Jump commit, and fencing. No synthetic
Error/retry-budget/cancel/discard change is attributable to internal rebuilding.

### Step 7 — Add the bounded invocation-start baseline optimization

Owners: existing fold/baseline machinery and actor-owned immutable status; reuse
`StatusCheckpointer` validation/fallback rather than adding persistent storage.

- Retain one candidate at the exact valid prefix before the outer invocation's
  `AgentInvocationStarted`, even within a multi-entry receipt. No entity-call
  baseline collection; no per-invocation persistent writes.
- Preserve the original candidate across retries. Drop/reject it on incompatible
  history/incarnation changes. Correctness cannot depend on its presence.
- Preserve batch finalization semantics, especially oplog-processor checkpoint
  pruning. Do not invoke the public finalizing fold separately on each segment.
- Try valid resident candidates, then the existing persisted checkpoint, then
  full reconstruction, all at step 2's horizon. Reuse `baseline_is_invalidated`:
  an index outside a Jump is not sufficient if incorporated history was removed.
- Initial scope excludes a separate resident snapshot slot and cold-reconstruction
  repopulation. Existing snapshot checkpoints remain supported.
- If exact capture needs disproportionate restructuring, use the user's permitted
  omission, record the concrete cost, and retain correct checkpoint/full-fold
  behavior. Do not invent another checkpoint system to keep the optimization.

**Exit tests:** asymmetric full-fold equivalence; baseline before/inside/after
newly skipped history; batched capture; repeated retries; consecutive invocations;
revert crossing invocation start; snapshot fallback; no resident candidate on
cold restart. Count entries read and allocations/retained bytes. An Arc is not
proof of zero cloning cost.

### Step 8 — Combined verification and documentation

- Run the affected state-actor, status, checkpoint, replay, and delivery suites,
  then the four integration schedules and relevant lifecycle/authority/result/
  stream/snapshot tests. Broaden according to changed shared contracts.
- Reattempt the reported held POST/wait crash scenario, with GET/tool controls
  and another agent remaining usable. Report any remaining reproduction limit;
  neither this experiment nor removing the assertion alone proves that incident
  fully fixed.
- Measure happy-path storage calls, status copying/retention, exceptional Jump
  reconstruction work, and extra Store reconstruction frequency. Require no new
  normal-path oplog scan/commit and no response dependency on status folding.
- Run scoped format/check commands. Update the executor walkthrough and durable
  execution guidance for both changed ownership contracts, per repository rules.
- Remove investigative prints and temporary hooks not needed by permanent tests.
  Keep necessary deterministic hooks in the existing test-hook mechanism.

**Completion criteria:** all regression schedules and affected checks pass; actor
projection and resident readiness have distinct owners; invalidation never leaks
through the status API; invalidated execution cannot publish new malformed
history; the storage policy, ordinary retained-call behavior, and early-response
contract remain intact. Report actual local/committed/pushed delivery state.

## 4. Commands and review boundaries

The child used these commands successfully to build the diagnostic (the two
adverse test cases intentionally exit 101 on the baseline):

```sh
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo build -p golem-cli --bin golem-cli
# In test-components/http-tests, with GOLEM_RUST_PATH set:
../../target/debug/golem-cli --preset release build --yes --skip-check
../../target/debug/golem-cli --preset release exec copy
# In repository root, run each separately and save its output:
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -p golem-worker-executor --test integration -- \
  gol710_cleanup_first --nocapture --report-time
```

Repeat the last command for `gol710_claim_before_cleanup`,
`gol710_already_issued`, and `gol710_retained_control`. Use matching toolchain,
lockfile, profiles, and environment for any performance comparison. Test names
may be improved during permanent-test integration; preserve the schedule matrix.

Keep tests, bounded projection work, actor/API work, transition ownership /
abandonment, and baseline optimization separately reviewable. Steps 5–6 form one
correctness unit and must not ship half-integrated. No new product decisions are
required. Normal implementation failures must be investigated, not worked around
by relaxing assertions, changing skipped regions, or weakening replay. Material
contradictory evidence reopens the design instead of being hidden by this plan.

## 5. Step-5 integration review: unresolved classification progress

The Oracle's integration review identified a gap in the ownership design, not
an implemented-code regression. Discovery of a potentially destructive scope
and selection of its recovery operation are different events. The existing
oplog scan can discover structural facts, but cannot select every rollback:
`ScopeReplayRecovery` and runtime `assume_idempotence` affect the policy, and
remote transaction recovery may query `is_committed` / `is_rolled_back`.
`set_idempotence_mode` mutates resident state without an oplog entry.

A caller awaiting live repair may hold exclusive access to the Wasmtime Store.
A retained scope's owner may need that same Store before it can claim and
classify its scope. Waiting for that unclassified scope then creates a possible
cycle: caller waits for classification; classifier waits for caller's Store.
Moving only Jump persistence to an owned task cannot resolve this earlier wait.
Nor can an unconditional rebuild resolve it: unchanged history may reproduce
the same schedule without any selected Jump.

Evidence checked locally:

- `replay_state/cursor.rs::try_get_oplog_entry` explicitly retains unclaimed
  Starts because their owner may need the reader's Store.
- `replay_state/resolution.rs::await_resolution_outcome` can return `Incomplete`
  at natural cursor exhaustion, independently of Live publication. A barrier in
  `switch_to_live` alone therefore does not guard every repair path.
- P2 `clocks/monotonic_clock.rs::Host::now` holds `&mut self` across durable
  session start and execution. Accessor scope preparation in `concurrent/call.rs`
  requires Store access and awaits before completing classification.
- Four existing unit tests passed using the step-4 test binary: retained
  incomplete Start publication, Jump pruning of retained Starts, overlapping
  scope adoption, and incomplete resolution across a retained Start. Output:
  `tmp/gol-710-step5-retention-contract.log` (`4 passed; 0 failed`). These confirm
  the retention contracts, not a real mixed-P2/P3 deadlock reproduction.

Required amendment before implementing the barrier:

1. Distinguish discovered potential obligations, independently owned
   classification, accepted recovery, and final live publication.
2. Select and accept recovery against a fixed generation/history revision/target
   before requesting Live. Acceptance installs its independent continuation and
   reserves its regions; it must not wait for its own obligation or completed
   body reconstruction fences.
3. Make `PendingReplayToLive` genuinely pending. Final publication must revalidate
   shared state after asynchronous attachment admission. Target equality alone
   is insufficient after history changes.
4. Preserve historical terminal/delivery/body-validation progress while gating
   new effects and incomplete-call repair. Keep the append-acceptance fence from
   step 6; a successful earlier publication does not authorize later stale writes.
5. Establish and test the missing scheduling invariant: before a Store-holding
   caller waits on another scope's decision, that classifier can run independently
   or the caller relinquishes the resource needed by it. Already-prepared scopes
   can transfer owned inputs; unclaimed retained scopes still require an earlier
   initiation/scheduling boundary. Its implementation is not yet settled.

Do not assume that preclassifying every scope from storage solves item 5, change
rollback policy, or silently introduce in-place handle re-admission. The Oracle
recommends extending independent ownership to classification, but the earlier
boundary for unclaimed scopes must be demonstrated before this is an executable
replacement for step 5. No new barrier has been installed and no new deadlock has
been reproduced. Step-5 Oracle/bug-finder implementation approval remains pending.

The separate step-4 export-fork test also still needs a default-stack comparison:
it passed with `RUST_MIN_STACK=16777216` after overflowing the default stack;
whether that failure is pre-existing has not been established.
