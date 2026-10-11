# GOL-616 handoff

Recommended next subissue: [GOL-616, expose resource-release and cleanup metrics](https://linear.app/golem-cloud/issue/GOL-616/21-expose-resource-release-and-cleanup-metrics). Research found it Todo, assigned to Kaur, with only Done GOL-615 as a formal dependency. The user authorized starting GOL-616 in a fresh pi-work session with this handoff. Confirm ownership and begin the bounded catalogue/plan checkpoint below. Obtain approval for that plan before implementation. This launch does not authorize unrelated repairs, publication, mutation execution or tracker updates.

## Start here

1. Verify the branch, HEAD, PR and current CI below. Read applicable AGENTS files, `testing`, `understanding-durable-execution` and `pre-pr-checklist` guidance.
2. Read GOL-442, GOL-616 and the [implementation runbook](https://linear.app/golem-cloud/document/gol-442-implementation-and-supervision-runbook-3b750b9da4fb). Its 2026-09-26 ownership override governs this work, rather than superseded lifecycle-driver designs.
3. Propose the exact metric catalogue, start/end events, bounded labels, source/test files and validation commands. Explain how unfinished age is collected without another polling task. Obtain approval before editing.
4. Before any needed local integration test, rebuild its required test components for the changed WIT contract as described next.

## Merge baseline and validation

- Worktree: `/home/ai/build/repos/worktrees/golem-oss/gol-442-unify-resource-limits/.pi/gol615-worker-delivery`.
- Branch: `gol-442-unify-resource-limits`; [PR #3894](https://github.com/golemcloud/golem/pull/3894).
- Merge baseline: `234e14e59c1c291ac980d3850ecb1092943b2362`. The monthly-limits rename and this handoff ship in the subsequent commit. The launch prompt supplies its exact published SHA; verify local/remote HEAD before work.
- Ordered parents: naming commit `f526a971f264ae163d7d8b8ff4b15ba57f7d097b`, then main `178f7e58435e7374ec4dbae062d48ea5f6eaacd6`.
- Local, remote and PR heads agreed after the non-force push. PR was open/draft and mergeable, with `mergeable_state=blocked`. The five source conflicts are resolved; this is not a readiness or merge approval.
- Exact merge-head Rust [CI run 37664261198](https://github.com/golemcloud/golem/actions/runs/37664261198) finished with failure: 55 jobs passed, 7 skipped, and `worker-tests-group5`, job `112948908687`, failed. Docs, Built-in Bash and CLA passed. No failure diagnosis or repair was performed. Check the subsequent naming commit's own CI separately; do not treat its source rename as a repair for this failure.
- Fresh merged integration compilation passed. Its Cargo-reported executable was `target/debug/deps/integration-52885f04167e0718`, SHA256 `b91f091c4dc3cdfe7c6886f1307a4659e6d09c177aec6f1df97597847ae88e82`. Rebuild and use the current Cargo-reported artifact after any source change.
- Native registration matched all 274 interruption cases. Total registrations were 1,748, including 57 new tests from main. All 69 interruption source files remained byte-identical to the validated naming change.
- Strict executor/test-utils all-targets Clippy, scoped Rust formatting, service builds, OpenAPI drift, generated-client compilation, docs generation/lint/format and independent merge source review passed.
- The merged-tree 274-case attempt exited 1 in fixture initialization before any case finished. The user explicitly chose CI instead of local fixture rebuilds for this merge. The earlier 274/274 pass belongs to the pre-merge naming tree, not this combined tree.

The full `generate-openapi` wrapper exited 105 after generation because the sandbox could not execute ESLint's `/usr/bin/env` launcher. Its generated YAML and MDX matched the staged source. Direct Bun generation and non-mutating lint/format checks, plus `cargo make diff-openapi`, passed without host or launcher edits. Keep the failed wrapper receipt distinct from these successful checks.

Preserve unrelated untracked `lefthook.yml`, SHA256 `89951797a15ac7f78e2e2979735a29377eace45f27154120d970dbf3b253669d`. Earlier one-command hook exceptions are consumed and never transfer to this session. Attempt normal hooks for any future commit; a new failure needs fresh approval.

## Monthly limits naming follow-up

The `monthly_limits.rs` module rename is committed with this handoff before the fresh session starts. All five affected library tests passed, along with integration compilation, scoped Clippy/format and eleven browser checks. Behavior and fixtures are unchanged. The earlier merge CI above does not cover this follow-up. The current integration executable at the same path has SHA256 `db88c935f5e56545d17123c9a0ee45d46d6966b8e8d5c97f88ad7ee6cda8d360`; the earlier hash is historical.

The proposed broader test-family name `execution_stopping` is not approved or implemented. `tests/api/interruption/` remains unchanged. Generic stopping supports several causes; monthly quotas are one use case. Durable quota exhaustion uses `Suspend`, while ephemeral exhaustion is a typed terminal resource failure. Do not conflate these with plain API `Interrupt` or immediate `Restart`.

## Local tests require rebuilt components

Incoming main changed `wit/deps/golem-agent/common.wit`. It adds `http-mount-details.file-response-headers` and the `file-response-header` record. This changes the guest metadata result signature. Old WASMs cannot be treated as compatible merely because they exist or passed before the merge.

The captured test attempt failed while prewarming `test-components/golem_it_tool_streaming_rust_caller_release.wasm` with:

```text
The component's golem:agent/guest interface does not match the expected type signature
... type mismatch with results
```

No interruption assertion ran to completion. Do not change runtime behavior, accept old signatures or weaken assertions to bypass this error.

When local tests are needed, get approval for a finite rebuild plan, then:

1. Load `modifying-test-components` and `modifying-wit-interfaces`; inspect the selected manifests, component AGENTS files and `test-components/build-components.sh` membership.
2. Build the merged `golem-cli` package with the warm Cargo home, one job and `--locked`. The old `target/debug/golem-cli` is not a fresh merged-tree build. Use the actual resolved executable path.
3. Build required components against the matching local SDK and synchronized WIT. Inventory the test-r dependency initialization too, not just the selected test's direct fixture. Even the interruption selection prewarmed the tool caller first.
4. For tool streaming, consider selecting provider and caller together. Caller-only selection previously failed explicit bridge matching; the paired local build remains unrun. Do not silently change the manifest or generator to evade that issue.
5. Verify the copied top-level WASMs, record hashes and preserve every intentional manifest/embedded-skill migration produced by rebuilding. Intermediate files or an existence cache are not freshness proof.
6. Rebuild the consuming test executable as needed, run the smallest selection once and require actual completed-test counts. Keep the failed initialization receipt; later passes do not erase it.

Build only required components. No full rebuild, SDK redesign, installation or fixture repair is preauthorized. If another prerequisite fails, report it before expanding the plan. This host has no usable rustup; use installed targets.

## GOL-616 scope

Expose accepted-stop stage and end-to-end durations, cleanup failures, unfinished count and oldest unfinished age using existing production telemetry and physical owners. A pending operation must remain visible without manufacturing a successful release or a finite release bound. Operator alerts and thresholds remain outside this implementation.

Preserve these contracts:

- Worker/WorkerInstance and the single retained `StopProgress` own acceptance and progress. Preserve the first winner, exact start/runtime/owner identities, writer receipts and retained cleanup health.
- A delivered signal, Suspended status or lifecycle notification is not physical release. Preserve FIFO status, causal-suffix replay, stream context and durable/ephemeral outcomes.
- Entered primary DB/KV work completes backend work and required durable finalization before typed interruption. Preserve transaction/cleanup ownership and external-effect ambiguity. Provider and cleanup latency may be unbounded.
- The existing permit-owned billing window closes before the permit drops. Instrument that sequence without moving it, adding another closure or stopping accrual on signal delivery.
- Ordinary telemetry works with all billing meters disabled. Add no quota-only timer, subscription, watchdog, stop coordinator, timeout policy or recovery protocol.
- Metrics must not extend Store/permit/Worker lifetimes, cancel work when observers drop, alter the oplog or turn a retained failure into success.

### Audit these existing owners first

| Source | What to inspect |
| --- | --- |
| `golem-worker-executor/src/metrics.rs` | Existing population/admission gauges and `golem_agent_filesystem_lifecycle_seconds`. Focused research found the filesystem helper definition but no recording calls. Prove actual wiring before reusing it. |
| `src/worker/mod.rs`, `accept_interrupt`, `StopPublication`, `StopProgressState` | Accepted leader identity and retained progress. Repeated signals are not new attempts. |
| `src/worker/invocation_loop.rs`, `unload_sealed_agent_ownership`, `UnloadCompletion`, `UnloadCleanup` | Runtime drop, sealed-work drain, verified deletion and final physical cleanup. A bounded failure notification can precede completion of retained cleanup. |
| `invocation_loop.rs`, `DisposalAccounting::release` | Existing window close and physical permit release. |
| `src/worker/monthly_limits.rs`, `ExecutionWindow`; `src/services/resource_usage_metering` | All-off behavior and retained settlement obligations. |
| `src/services/agent_filesystem/lifecycle/mod.rs` | Verified deletion, cleanup failure and retry ownership. |

The paths abbreviated with `src/` are under `golem-worker-executor/`. Genuine monthly policy keeps its production name; `tests/api/interruption/` names the broader behavior.

### Acceptance to propose and prove

- Define every start/end fact. Keep local detection delay separate from accepted-stop-to-release time; do not invent a global exhaustion timestamp.
- Use duration seconds and GOL-616's finite boundaries `0.01, 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, 60, 120, 300`, plus exporter overflow. Do not change shared default buckets. Thirty seconds is not a release deadline.
- Define unfinished count/oldest age, identity and process/reset lifetime. Permanently pending work has no completed duration but remains observable.
- Separate successful physical release, failures and retained unfinished work. Prevent duplicate observations across repeated signals, retries, observer drops and joins.
- Use bounded stage/outcome/reason labels, never account/agent/invocation IDs, paths, URLs or arbitrary error strings.
- Test actual registry/export behavior with existing cleanup controls and injected time. Cover success, slow/pending cleanup, failure, duplicate signals and dropped observers without copied state machines, sleeps or longer deadlines.

Reuse the existing colocated unload/deletion/deadline/observer tests in `worker::invocation_loop::tests` and all-off settlement tests in `worker::monthly_limits::tests`. Propose exact filters and new metric tests after designing the catalogue. A small real composition candidate is the durable/ephemeral silent-TCP pair in `api::interruption::host::p3::tcp`, after rebuilding prerequisites. Read the `testing` skill for test-r filtering; omit the displayed `integration::` prefix and reject zero-selected runs.

Before heavy work consult `/run/pi-tools/agent-pool-status`. Use `CARGO_HOME=/home/ai/build/tmp/pi-768193-16755/cargo`, one compiler job, the existing target directory and separate one-thread test execution where needed. Keep one source writer and one heavy runner. No broad `cargo make test` or slow mutation programme is authorized.

## Other issues and retained limits

- GOL-620 was In Progress under Kaur. Reconcile ownership and delivered host-wait coverage before taking over or claiming closure.
- GOL-619's Todo cancellation text is stale relative to the authoritative completion-only agreement and delivered witnesses. Reconcile the tracker only with permission; do not reimplement pending-provider cancellation.
- GOL-621 is final combined verification after GOL-616 and remaining prerequisite/coverage reconciliation. GOL-616 does not authorize that whole programme.
- Earlier owner-election setup and CI failures remain unexplained. Pre-merge green tests and this source merge do not establish their causes.
- Strict host-fixture WebSocket Clippy previously failed separately. Native DB waits, every operation/race, entity pending-call survival and concurrent P3 siblings are not universally proved. No arbitrary external exactly-once or bounded-release claim is justified.

Return metric-to-criterion/test evidence, exact commands/counts/exits, export buckets and label/reset definitions, source identities, review findings and residual limits. Publication, hook exceptions, mutation execution, CI reruns, readiness changes and issue closure require fresh permission.

## Evidence

- Publication/merge receipts: `/home/ai/build/tmp/pi/gol-442/interruption-organization/phase2-65a449/publication/current-main-178f/`.
- Pre-merge naming validation: `/home/ai/build/tmp/pi/gol-442/interruption-organization/phase2-65a449/validated-handback-v1.md`.
- Next-issue research and raw tracker receipts: `publication/handoff-research/recommendation.md` under that same phase-two evidence root.
- Authoritative completion-only scope: `/home/ai/build/tmp/pi/gol-442/gol-619/implementation-handoff.md`; historical witness evidence in `validated-handback.md` alongside it.

This handoff ships with the monthly-limits naming commit. It has no GOL-616 implementation/test evidence of its own. The fresh session starts from the recorded decisions and approved planning checkpoint, rather than repeating the earlier implementation or reopening deferred repairs.
