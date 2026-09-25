# Capability-sensitive guest exports

The shared sbt/Mill auto-registration generator owns the Scala.js export roots.
It selects full agent, tool, and middleware dispatch only for **registered
implementations**. Client-only traits and middleware projections do not expose
local agents or tools. Enable generation with the existing `golemBasePackage`
setting (sbt or Mill); manually registering a
class without generating an entry module does not export it.

Every generated entry retains the same agent, tool, middleware, and snapshot
export surface. Absent capabilities return synchronous empty discovery or
explicit errors. Without agents, save returns an empty binary snapshot and load
rejects with a string error. No component role, alternate world, or runtime
selection is introduced. All components use the same full guest runtime.

## Linked integration checks

From `sdks/scala`, after generating the guest runtime resources:

```sh
sbt -batch '++3.8.2; emptyAutoRegisterFixture/fullLinkJS; toolExportsFixture/fullLinkJS; agentExportsFixture/fullLinkJS; middlewareGuestLinkFixture/fullLinkJS; mixedExportsFixture/fullLinkJS; clientExportsFixture/fullLinkJS'

node test-capability-exports/check.mjs empty test-empty-auto-register/target/scala-3.8.2/golem-scala-empty-auto-register-fixture-opt/main.js
node test-capability-exports/check.mjs tool test-capability-exports/toolExportsFixture/target/scala-3.8.2/toolexportsfixture-opt/main.js
node test-capability-exports/check.mjs agent test-capability-exports/agentExportsFixture/target/scala-3.8.2/agentexportsfixture-opt/main.js
node test-middleware-guest-link/check.mjs test-middleware-guest-link/target/scala-3.8.2/golem-scala-middleware-guest-link-fixture-opt/main.js
node test-capability-exports/check.mjs mixed test-capability-exports/mixedExportsFixture/target/scala-3.8.2/mixedexportsfixture-opt/main.js
node test-capability-exports/check-client.mjs test-capability-exports/toolExportsFixture/target/scala-3.8.2/toolexportsfixture-opt/main.js test-capability-exports/clientExportsFixture/target/scala-3.8.2/clientexportsfixture-opt/main.js
```

Node 24 checks the actual linked entry modules, synchronous discovery, externally
called guest exports, middleware underlying dispatch, error behavior, snapshot
exports, and static import pruning. Unexpected ambient host calls fail. The
client-only fixture exercises generated typed tool RPC through a host stub into
the real provider entry; it must import tool RPC while keeping local discovery
empty. These checks do not test platform permissions, durable RPC, or scheduling.
The existing `core/testOnly golem.runtime.SnapshottingSpec` exercises nonempty
snapshot save/restore, including config and principals.

## Size and latency

Use `fullLinkJS`, not `fastLinkJS`, for comparisons. Preserve the baseline
bundles before editing the runtime/generator. For each baseline and candidate:

1. Record bundle bytes and static imports.
2. Inject with `wasm-rquickjs inject-js --input <full-guest-runtime.wasm>
   --output <component.wasm> --js <main.js>` using **identical runtime bytes**.
3. Run `wasm-tools validate --features all <component.wasm>` and compare
   `wasm-tools component wit` output with the full runtime. It must be identical.
4. Record raw and `wasm-tools strip -a` bytes. Repeat after
   `wasm-rquickjs optimize --input <component.wasm> --output <preinit.wasm>`.
   Preinitialization embeds QuickJS heap state, so measure it separately.
5. Record the runtime itself, raw and stripped. That is a fixed cost, not a
   saving from Scala.js dead-code elimination. The empty Scala bundle also
   retains a timezone database registered by the existing model dependency.

`node test-capability-exports/bench.mjs <tool-main.js>` measures module
load/registration and a 64 KiB echo invocation, with 100 warmups and 1000 checked
samples. Run fresh Node processes, interleaving baseline and candidate, several
times. This is a JS-boundary microbenchmark, **not Golem/QuickJS latency**.
Platform external-tool and guest-side RPC integration still need the built
Golem executable and its test dependencies.
