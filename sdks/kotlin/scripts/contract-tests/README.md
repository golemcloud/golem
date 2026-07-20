# Contract-test harness

`../native-contract-tests.sh <workdir>` scaffolds a Kotlin app, swaps in `ContractProbeAgent`,
builds/deploys it to a locally built golem server, and invokes one probe per capability to prove
the compiled-Kotlin ⇄ host ABI boundary works.

**Scope: contract-only.** Each probe proves a host call crossed the boundary and returned the
expected *shape* without trapping — not that the value is functionally correct. Durability's
persist-then-replay behaviour is out of scope (the durability probe proves only that the
durable-function imports marshal). Agent snapshotting is covered by its own dedicated test in the
native-agent-snapshotting work, so it is intentionally not re-probed here.

Each probe runs on its **own durable agent** (keyed by the method name) so a wasm trap in one probe
wedges only that worker and can't cascade false FAILs onto later probes. Kotlin agents use
`golem-cli`'s fallback TypeScript literal syntax for invoke args (records `{ field: value }`, lists
`[a,b,c]`, option-some as the bare value).

Prerequisites (same as `native-e2e.sh`): SDK/KSP/gradle-plugin published to mavenLocal, and
`golem`/`golem-cli` built from this branch. On a machine whose default `java` is < 17, point
`JAVA_HOME` at a 17+ JDK for the gradle build. Exit 0 iff every probe passed.

Capabilities probed (10): agent model, type mapping (lower + lift), host API, oplog, retry DSL,
transactions, guards & checkpoint, secrets, context/tracing, durability.
