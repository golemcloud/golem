# GOL-710 implementation plan

**Current disposition: ownership-filtered replacement rejected.** Section 6 is
an investigation record, not an executable approved plan. Observations from HTTP,
streaming invocations, and entities can influence operations outside their ownership
tree. The user rejected extending ownership filtering as a replacement for deleted
regions. The uncommitted behavioral draft must not ship. This does not invalidate
the separately approved status-actor correction in steps 1–4.

Review of existing entity rollback found a concrete missing isolation invariant:
filesystem-incapable tools can publish raw stdout before their entity terminal or
an enclosing atomic block ends. `HostToolOutputWriterWithStore::write` writes to
an in-memory attachment; `create_output` exposes that attachment directly, without
a durable consumer journal. Existing `clean_stdout_then_trap` coverage proves
early observation, not atomic rollback safety. Recovery can delete the entity's
input observation while retaining caller/sibling work derived from its output.
The Oracle independently confirmed the code path. The focused experiment below
now reproduces recovery blocking with a foreign positional entry.

Required focused experiment: an incapable entity reads a changing value inside an
atomic block, publishes it on raw stdout, and parks before atomic/entity completion.
The caller observes it and completes a sibling invocation whose input contains that
value. Crash and reconstruct after both are persisted. Check whether entity-local
rollback regenerates different bytes against the retained sibling invocation.
Ordinary result publication and capable-tool guest-output staging do not cover this
path. Durable stream journals are a separate mechanism and must not be assumed to
protect raw attachments. No new general recovery design is approved by this review.

### Entity causal rollback experiment and remaining decision

The rejected HTTP/transaction ownership draft has been removed. The atomic suffix
replacement is now implemented locally on top of the approved status-actor fix.

`incomplete_entity_atomic_rollback_recovers_dependent_sibling` runs a provider
atomic block that reads `first`, publishes raw stdout, and parks before completion.
The caller completes a sibling from those bytes before the crash. Recovery reads
`second`. The pure-echo sibling control passes both recovery and an actual executor
drop/restart followed by `replay_probe`, with two external reads total. An idle
`simulated_crash` is not a reconstruction: the lifecycle intentionally ignores it.

Adding `get_oplog_index()` to the sibling makes recovery time out. In the observed
history, the changing entity starts at 12 with atomic Begin 15; sibling Start 35,
NoOp 44 and End 46 remain outside the entity-owned Jump masks. The provider retries
and reads `second`, but the old sibling's positional obligation blocks recovery.
This establishes blocking, not an observed wrong result or explicit divergence
error. The weaker control can drain and later discard unclaimed closed Start/End
records; it therefore does not establish causal consistency.

Commands/evidence:

- Build/copy the tool-streaming fixtures through `golem-cli --preset release`.
- `cargo test -p golem-worker-executor --test integration -- incomplete_entity_atomic_rollback_recovers_dependent_sibling --report-time --nocapture`
- Pure-echo real-restart control: `tmp/gol-710-entity-real-restart.log`, 1 passed.
- Positional regression: `tmp/gol-710-entity-noop.log`, 1 failed after the 60-second
  recovery deadline; external reads = 2. This is an intentional red regression,
  captured before the fix. Oracle approved the regression; bug-finder reported no
  bugs in the test/fixtures.

The implemented atomic stage moves suffix normalization before Store construction,
removes entity admission-time masking and the primary Begin-time destructive Jump,
and reloads startup metadata after committing the cut. Oracle approved the planner
and integrated stage; bug-finder reported no bugs in both stages. Verification:

- `cargo test -p golem-worker-executor --lib -- durable_host::replay_state::tests --report-time`:
  166 passed (`tmp/gol-710-atomic-integration-unit.log`).
- The formerly failing causal guest regression: 1 passed, including actual executor
  restart, external reads remaining at two, and a Jump covering the old sibling's
  Start through End (`tmp/gol-710-atomic-integration-guest.log`).
- Sync/async primary atomic tests, typed tool atomic restart, and owner trap retry:
  4 passed (`tmp/gol-710-atomic-regressions.log`).
- Scoped Rust formatting and `git diff --check` pass. The changed walkthrough
  paragraph was rendered and inspected; durable-execution skill text is aligned.

This approval does not close HTTP/transaction runtime-Jump repair, invocation-start
status baseline optimization, or final combined verification. Nothing is pushed.

The reviewed direction is to normalize ordinary incomplete atomic regions before
constructing any Store or admitting replay claims. After old-generation terminal
writers are drained, use a fixed committed horizon H; find the earliest unmatched
Begin B; move B backward to a fixed point across earlier atomic regions whose Ends
would be erased; commit one suffix Jump `[B.next(), H]`; reload authoritative status
and snapshot selection before instantiation. Pair Ends by begin index, not a global
nesting stack. Ignore already-skipped work and recovery-only hints when deciding
whether another Jump is needed. Remove entity claim-time masking and prevent a
second destructive atomic cut after claims have started. Starts before the cut
whose terminals are removed become incomplete and reconstruct normally.

The user resolved the transaction question: resetting an outer atomic region reruns
the work within its abandoned suffix, including transaction commits. Do not preserve
settled foreign transactions specially, introduce an outcome journal, or add a new
overlap rejection policy. Rollback does not undo an external database commit; the
retried region may execute transactional work again. Existing durable RPC identity
and peer-specific idempotency contracts remain unchanged. This is the intended retry
semantics, not a blocker to the suffix normalizer. HTTP/transaction runtime recovery
otherwise remains a separate stage.

2026-10-02. **Implementation plan — ownership-based replacement for steps 5–6.** The original final review
returned “APPROVE — no blockers to implementation” and required no further
pre-implementation experiment. Approval covers this handoff, not correctness of
an unimplemented fix or permission to ship partial work. The step-5 integration
review subsequently identified an unresolved Store-scheduling dependency; see
section 5 below. Do not implement an all-scope publication barrier as currently
written without resolving that dependency.

Implementation is in progress locally. Steps 1–4 have Oracle and bug-finder
approval and are checkpointed in local commit `4fe88a2c9`.
The behavior-preserving entity rollback extraction also has both approvals and
166 passing replay-state tests; it is committed locally as `ea67d4f50`.
Behavioral replacement of steps 5–6 and steps 7–8 remain outstanding.
Nothing has been pushed or deployed. Section 6 supersedes the broad-suffix
abandonment design in the original steps 5–6 and the proposed barrier in section 5.
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

## 6. Ownership-based recovery revision

The user selected the entity rollback pattern for HTTP, transactions, and atomic
regions, keeping implementation in this checkout and separating refactoring from
behavior changes. The original entity work in PR #3914 installs discontiguous
owned masks before entity body claims, rather than deleting a physical suffix.
Keep the actor projection correction independent of execution-recovery ownership.

### A. Extract the existing planner — complete

Moved `entity_atomic_rollback_regions` unchanged from `cursor.rs` into
`replay_state/rollback.rs`; preserved signature, call timing, raw ancestry before
deleted-entry filtering, exclusions, and chunking. Oracle approved the actual
diff; bug-finder `gol710-rollback-extraction` run 1 was clean.

Verification: `cargo fmt -p golem-worker-executor -- --check` passed;
`cargo test -p golem-worker-executor --lib -- durable_host::replay_state:: --report-time`
reported **166 passed, 0 failed**. Log:
`tmp/gol-710-rollback-extraction-tests.log`.

### B. Establish complete ownership and recovery admission — blocked on causal replay

The ownership planner is implemented as an uncommitted draft, not an approved
behavioral stage. The initial 168 replay-state tests passed, but they establish
record selection, not safe resumption. The Oracle rejected propagating primary
scope-local Live: primary authorization, card-event synchronization, and completion
ordering use Store-wide liveness. The attempted local admission helper has been
removed. The remaining draft still uses global Live switching and must not ship.

- Correct top-level custom-invocation entity attribution using existing parent
  fields, without confusing the entity root with an active custom parent used
  for logical invocation IDs. Audit observational ownership against the existing
  custom-subtree replay rules before extending the projection.
- Plan HTTP/transaction rollback under the existing synthetic scope Start,
  retaining that root and preserving foreign sibling records. Transaction
  selection includes the paired initial begin and subsequent protocol markers.
- Install the selected mask before the scope's descendants can claim deleted
  history. Accepted commit-and-install work must survive caller cancellation and
  finish without holding the cursor/Store across persistence waits.
- Replace global replay-to-live switching on these recovery paths with a
  recovery protocol validated against the causal case below. Smaller regions alone are insufficient:
  `switch_cursor_to_live` clamps shared replay and releases sibling resolvers.
  Do not substitute primary scope-local Live or weaken authorization assertions.
  Audit child admission, authorization, retry policy, scope close, and span cleanup;
  preserve unread as well as retained independent siblings.
- Keep method/idempotence, reconstructable request-body, and remote transaction
  outcome checks. Missing-Start recovery remains a separate case without a
  recorded scope root. Do not interpret selective rollback as permission to
  repeat arbitrary non-idempotent effects.

### C. Extend pre-body atomic rollback to the primary invocation

Install the plan before primary guest replay, not when guest code eventually
reaches `mark_begin_operation`. Preserve independent pre-existing entity trees;
discard the complete descendant tree of an entity belonging to the abandoned
primary interval. Explicitly classify worker-wide records; lack of attribution
is not proof of primary ownership. Preserve ancestry through partial Jump commits.

The user selected **persisted initiation-time membership** for this separate
stage. A tool can capture
its logical key and initiation context before `BeginAtomicRegion`, but persist
its Start afterward. Existing resident leases use initiation-time membership;
the simple historical projection uses durable record order. Do not silently
equate these. Capture membership at initiation, persist it with the operation's
durable attribution, and use it during cold reconstruction; do not infer membership
from physical Start order or wait for every asynchronous effect to finish.
Implement only after HTTP/transaction recovery is working and separately reviewed.
No new primary atomic behavior has been implemented or approved.

### D. Verify before replacing the adversarial baseline

Required coverage includes unread/retained siblings, overlapping transactions,
pre-existing and newly owned entities, custom/observational descendants, pre-begin
Start with post-begin terminal/delivery, delayed Start persistence across atomic
begin with stable effect keys, cancellation after acceptance, partial multi-Jump
commit, and two reconstructions. Run the four original HTTP/clock schedules and
count external effects. Obtain Oracle and bug-finder approval for each behavioral
stage, then continue the baseline optimization and combined verification.

### E. Causal replay review checkpoint

Ownership is not causal independence. A possible guest schedule is:

```text
open transaction/scope R
await owned operation Q; observe its result
await unrelated clock C
crash before R closes
```

An ownership-only mask removes Q's Start, result and delivery but keeps C's
Start, result and delivery. Waiting for ordinary replay exhaustion before repairing
Q is not generally valid: the guest may need Q's result before initiating C,
while replay cannot exhaust until C is observed. Reconstructing the Store does
not remove that dependency. Likewise, appending Q's new delivery after the
retained C delivery needs a durable ordering explanation for the next reconstruction.

The Oracle recommends ordinary Jump reconstruction after committing nonempty
ownership masks rather than primary scope-local Live. That is only a candidate:
the empty-mask continuation still needs a causal-progress design. An empty mask
must not cause another reconstruction forever. Transaction recovery also must
handle its original Begin being deleted before it attempts a positional Begin read.

Bug-finder run 1 (`gol710-http-ownership`) surfaced
`retained-sibling-delivery-blocks-scope-recovery` as SPEC_CONFLICT. Its provisional
test failed, but did not call the ownership planner or the production scope-open
lookup; the latter uses a non-consuming scan, not `await_resolution_outcome`.
Therefore that failure is not accepted as a production regression or proof that
classification blocks. The concern remains open; the provisional test was replaced
with `scope_rollback_does_not_release_retained_sibling_delivery`. This checks the
actual planner and natural-tail wait on history containing an owned result/delivery
before a foreign result/delivery, and verifies that only the foreign guest delivery
releases the tail. It is a replay-mechanics test, not a guest-level reproduction.

Before claiming this stage approved, establish a guest-level causal schedule and
choose a recovery protocol that preserves its observations or accounts for its
dependencies. Do not silently turn permitted overlapping reads into errors, delete
all foreign history, or bypass completion ordering. No final protocol for this case
has been approved; atomic implementation remains deferred behind this stage.

Follow-up Oracle review distinguishes two protocols:

- **HTTP partial-response recovery:** retain and replay already-observed body
  results, then reconstruct the transport at the consumed offset at the first
  missing result, after ordinary live admission. P2 repairable scoped calls and
  P3 response-body Range/full-response resumption provide local precedents.
  P3 scope-open and demand-stream incomplete-call handling do not yet compose
  these mechanisms. Preserve method/idempotence and request-body restrictions;
  do not resend merely because a replayed body is dropped. This is a candidate
  for a bounded implementation, not verified behavior in the current draft.
- **Transaction reconstruction:** replaying retained SQL results does not restore
  database state. `create_replay` creates a closed handle and replayed SQL does
  not execute. Reexecuting statements in a new transaction can produce different
  results (for example, `INSERT ... RETURNING id` returns 42 instead of the
  recorded 41). Assuming idempotence does not imply result equality.

The outstanding user-visible choice is whether transaction recovery may replace
old observations and abandon causally dependent guest work, or must retain those
observations and report a recovery conflict when a replacement transaction cannot
reproduce them. Preserving fresh-transaction retry semantics is the recommendation;
that requires more than ownership attribution to distinguish dependencies from
independent siblings. A mismatch-failure policy would be a new restriction, not a
silent implementation simplification. A real transaction fixture with deliberately
changing returned values remains necessary before final implementation approval.

Current verification after replacing the provisional test:
`cargo test -p golem-worker-executor --lib -- durable_host::replay_state:: --report-time`
reported **169 passed, 0 failed** in
`tmp/gol-710-ownership-delivery-tests.log`. Scoped formatting and `git diff --check`
passed. This verifies replay/planner mechanics only; neither review approved the
HTTP/transaction behavioral stage. All work in this stage remains uncommitted.
