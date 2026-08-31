---
name: golem-scala-code-generation
description: "Develops shared Scala source generation for the Golem sbt and Mill plugins. Use when changing source discovery, validated codegen IR, generated RPC/tool clients, auto-registration, or plugin generation hooks under sdks/scala."
---

# Golem Scala Code Generation

Keep semantic generation in `sdks/scala/codegen/` and the sbt/Mill integrations thin.

## Follow the surrounding pipeline

The current shared flow is:

```text
Scala source -> SourceDiscovery -> validation / semantic IR -> source renderer
             -> GeneratedFile + warnings -> sbt or Mill writes/formats files
```

`CodegenPipeline` rejects discovery and projection errors before emitting files. `AgentSurfaceIR` is the agent interchange model; `ToolProjectionIR` resolves and validates tool command shapes shared by ordinary tool and middleware renderers. Preserve this separation when adding behavior: do not independently rediscover semantics in each renderer or build plugin.

There is no repository-wide rule that generation must be AST-only. Follow the implementation being changed:

- discovery and type analysis use scalameta with the Scala 3 dialect;
- auto-registration builds scalameta `Source` trees and emits `.syntax`;
- RPC, tool, and middleware generators render validated IR with structured source builders;
- parse user-supplied type syntax before transforming it; do not concatenate unchecked semantic inputs.

Prefer typed, validated IR and small shared rendering helpers over either brittle ad-hoc templates or an unnecessary AST rewrite.

## Shared-library constraints

The same `codegen/src/main/scala` sources are consumed by:

- sbt 1.x on Scala **2.12.21**;
- the repository build and Mill plugin on Scala **3.8.2**.

Therefore avoid Scala-3-only source syntax in shared code. The current Mill consumer fixtures
compile generated application code with Scala **3.3.7**; this fixture setting is not the codegen
host version and should not be generalized to every Scala.js project.

Keep shared APIs deterministic and free of sbt/Mill types and file I/O. Return relative paths, contents, diagnostics, and errors. Build-tool wrappers own source collection, logging, managed output directories, formatting, caching, and task wiring.

## Integration points

- `codegen/pipeline/CodegenPipeline.scala`: shared orchestration and fail-fast validation
- `codegen/discovery/SourceDiscovery.scala`: annotation/source discovery
- `codegen/ir/`: stable agent surface IR and codec
- `codegen/rpc/`: agent RPC, tool RPC, projection, and middleware generation
- `codegen/autoregister/`: implementation registration
- `sbt/.../GolemPlugin.scala`: sbt tasks and `Compile / sourceGenerators`
- `mill/.../GolemAutoRegister.scala`: Mill generated-source tasks

Both plugins must remain behaviorally aligned. Check their existing task names and output handling before adding a hook rather than copying an old example.

## Current generated RPC surface

Generation is mode-aware:

- durable clients expose `get` and phantom constructors; config variants exist only when config fields exist;
- ephemeral clients expose `newPhantom` (and its config variant when applicable), not durable `get`;
- durable `apply` returns `Future[A]`, while ephemeral `apply` returns `Future[InvocationResult[A]]`;
- durable `cancelable` returns `(Future[A], CancellationToken)`, while ephemeral returns `Future[CancelableAsyncInvocation[A]]`;
- ephemeral trigger/schedule operations return metadata-bearing receipts; durable variants return `Unit` or a cancellation token as defined in `RpcCodegen.scala`.

Treat `RpcCodegen.scala`, `AgentClientRuntime.scala`, and `InvocationMetadata.scala` as authoritative for exact signatures.

## Verification

Start with the affected generator tests and compile a consumer when output contracts change:

```bash
cd sdks/scala
sbt "++3.8.2; codegen/test"
sbt "++2.12.21!; codegen/test; sbtPlugin/test"
sbt "++3.8.2; testAgents/fastLinkJS"
```

For Mill integration changes, use the tasks documented in `sdks/scala/mill/README.md` and the fixtures in `mill/build.mill`. Run project-scoped `scalafmtCheckAll` where possible. Integration tests are required only when behavior reaches generated applications or the host runtime.
