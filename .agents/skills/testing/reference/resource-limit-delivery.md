# Reuse-first resource-limit delivery

Use this before inventorying or implementing pending-host quota extensions. Apply the current user scope and issue contract, then the repository rules. The inventory records evidence; it does not create acceptance requirements.

## Freeze a bounded plan

Before source edits, record the owned supported operation classes, existing stop path and passing tests, concrete integration gaps, required validation, and non-blocking residuals in the issue checkpoint. Separate normal supported use from unsupported or deliberately constructed states. A fixture method exercising an existing public API is normal test work, not proof that the API is unsupported.

Delivered agent deletion is the stopping baseline. Monthly extensions inherit the Worker-owned monitor, shared resource policy, typed signal, accepted-stop driver, invocation-loop cleanup, permit-owned metering window and ordinary reconstruction. See [monthly interruption and recovery](../../understanding-durable-execution/SKILL.md). Deletion removes an agent; monthly suspension preserves durable work. Neither difference requires a new stop system.

Implement only missing subscriptions or ownership integration in the owned host adapters. A new coordinator, whole-invocation cancellation wrapper, recovery protocol, metering mechanism or stronger arbitrary-stop contract requires explicit user approval. Reuse the recorded architecture decision after a restart instead of repeating the deletion investigation.

## Triage the inventory

For each owned class, name the existing signal/cleanup path and evidence, then classify:

- **Covered:** existing tests and a source trace exercise the relevant shared mechanism. Preserve their exact evidence limits.
- **Ordinary integration gap:** a supported pending operation fails to observe the existing signal or violates required ownership/outcome handling. Establish one focused witness and fix that boundary.
- **Residual:** an independent defect, unsupported path, contrived case, backend limitation or unverified combination. Describe its impact and evidence; missing dedicated coverage alone is not a demonstrated production failure.
- **Nonparking:** the current implementation returns immediately or exposes no pending operation. Cite the source rather than manufacture a pending fixture.

Use a separate issue for unrelated defects. Choose its urgency from demonstrated impact and normal-use likelihood, not the worst theoretical outcome. A backlog defect does not automatically become a prerequisite for the quota extension. A residual is not a passing test, so narrow the coverage claim instead of claiming universal interruption or release bounds.

Stop and ask before adding a subsystem, an unrelated replay repair, new resource dimensions, a privileged test environment or a larger test programme outside the frozen plan. If a focused fixture turns into an open-ended reachability investigation, report what is known and return to that plan.

## Test the integration, not every internal await

1. Reuse ordinary interruption/delete and quota tests first. For a real gap, run one bounded supported-path case showing the actual pending operation and targeted accepted stop. Use an in-process peer or the framework's backend seam. An open Start, elapsed time or silent peer alone is not proof of a particular provider-internal await. Passive native-Pending observation is useful when attribution matters, not a mandatory new hook for every await.
2. Add the existing typed select only around the interruptible backend wait. Keep durable completion, transactions, sealing, drain and mandatory cleanup under their existing owners. Preserve the selected outcome and incomplete durable handles. Do not invent End/Cancelled or abort the entire unfinished invocation.
3. Verify physical cleanup and normal durable continuation or ephemeral terminal failure. Signal acceptance alone is not permit release. Count provider effects where the existing contract defines them; do not promise new external exactly-once semantics.
4. Reuse the established resource/mode and ordering tests for shared paths. Run extra cells where an adapter changes those policies or the bounded issue contract requires them. Do not create a six-cell matrix and all race permutations for each nested helper by default. Zero unreserved fuel is not prepaid-fuel exhaustion; storage classes remain independent. Scripted allocation proves orchestration, not native XFS. Representative monthly-memory evidence is sufficient where the issue explicitly delegates other resource dimensions to the shared contract.
5. Run affected tests after every test edit. Once the source cluster is stable, run one affected regression group, scoped format and strict Clippy. Add a separate check only for consumers/targets not already validated. Save commands, exit codes and test results; distinguish wrong test oracles, stale fixtures and build failures from behavioral REDs. Contain test-request tasks without taking or cancelling Worker-owned settlement.

Completion-first and publication-before-subscription controls test different orderings. Reuse shared controls where applicable; neither establishes a simultaneous native-ready/stop-ready tie. Add an operation-specific ordering test only for a changed boundary or an actual uncovered requirement in the bounded plan.

## Build, review and finish

Keep one source writer and coordinate heavy Cargo jobs. Before every heavy command use `/run/pi-tools/agent-pool-status`, size aggregate compiler jobs for anonymous memory/headroom/pressure, and sample long builds. Set test threads separately, keep a stable warm cache/target, and build only selected fixtures. Never use `cargo make test` as a shortcut.

Update the durable-execution skill and walkthrough for actual behavior changes. Browser-check changed sections at a stable documentation boundary. Give reviewers the frozen plan and current user clarifications. Ask them to assess the delivered contract, not expand it into every hypothetical await, fault injection or unrelated recovery issue. New acceptance demands require approval before another implementation round. Rerun only checks affected by a correction.

Ready for final review means required normal-operation integration is delivered, changed tests pass, known limits are explicit, and the finite checklist is complete. A phase count or an entirely green exhaustive inventory is not the criterion. Retain independent final reviews, ticket-specific explicit mutation approval and the requested commit/push gates.

Handoffs must link the accepted architecture, current scope, required remaining work, non-blocking residuals and exact final evidence. Resume from those decisions rather than restart design.
