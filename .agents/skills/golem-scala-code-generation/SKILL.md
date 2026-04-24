---
name: golem-scala-code-generation
description: "Generating Scala code in the Golem Scala SDK. Use when adding code generation steps, build-time source generators, scalameta AST construction, or sbt/Mill sourceGenerators to the sdks/scala/ subtree."
---

# Golem Scala Code Generation

Guidelines for writing Scala code generators in the Golem Scala SDK.

## Core Principle: Scalameta AST, Never String Templates

Always construct generated code as **scalameta AST nodes** using quasiquotes (`q"..."`, `t"..."`, `source"..."`, `param"..."`). Never use string interpolation or text templates to produce Scala source files.

```scala
// ✅ Correct — typed AST
val tree = q"""
  object $name {
    def register(): Unit = {
      ..$registrations
      ()
    }
  }
"""

// ❌ Wrong — string interpolation
val code = s"""object $name {
  def register(): Unit = { ... }
}"""
```

## Shared Codegen Library (`sdks/scala/codegen/`)

All build-time code generation logic lives in `sdks/scala/codegen/`, a pure (no ZIO, no sbt, no Mill) Scala library that cross-compiles to **Scala 2.12** (for sbt) and **Scala 3.3.7** (for Mill). Both plugins depend on this shared library.

### Cross-compilation constraints

Because the library must compile under Scala 2.12:

- Use `import scala.meta.dialects.Scala213` for the implicit dialect needed by quasiquotes and `.parse[T]` calls.
- For parsing with a specific dialect, use `dialects.Scala3(code).parse[Source]` (explicit dialect application) rather than `implicit val d: Dialect = ...` which causes ambiguity.
- Use `parseMeta[T](code)(implicit parse: Parse[T])` helper pattern for snippet parsing.
- Avoid Scala 3-only syntax in shared code.

### Type and term references

Parse dotted strings into scalameta AST nodes for use in quasiquotes:

```scala
private def parseMeta[T](code: String)(implicit parse: Parse[T]): T =
  Scala213(code).parse[T].get

private def parseTermRef(dotted: String): Term.Ref =
  parseMeta[Term](dotted).asInstanceOf[Term.Ref]

private def parseType(tpe: String): Type =
  parseMeta[Type](tpe)

private def parseImporter(dotted: String): List[Importer] =
  parseMeta[Stat](s"import $dotted").asInstanceOf[Import].importers
```

### API pattern

Generators should expose a **pure, effect-free API** that accepts source text and returns generated outputs + diagnostics:

```scala
object MyCodegen {
  final case class GeneratedFile(relativePath: String, content: String)
  final case class Warning(path: Option[String], message: String)
  final case class Result(files: Seq[GeneratedFile], warnings: Seq[Warning])

  def generate(inputs: ...): Result = {
    // 1. Parse/scan inputs
    // 2. Build scalameta AST via quasiquotes
    // 3. Pretty-print via .syntax
    // 4. Return GeneratedFile with relative path + content
  }
}
```

The plugin wrappers (sbt/Mill) handle file I/O, logging, and build-tool integration.

## Build Integration Pattern

### sbt Plugin (sdks/scala/sbt/)

The sbt plugin `GolemPlugin` is an `AutoPlugin` compiled as part of the meta-build via `ProjectRef` in `project/plugins.sbt`. It hooks into `sourceGenerators`:

```scala
Compile / sourceGenerators += Def.task {
  val inputs = scalaSources.map { f =>
    MyCodegen.SourceInput(f.getAbsolutePath, IO.read(f))
  }
  val result = MyCodegen.generate(inputs)
  result.warnings.foreach(w => log.warn(s"[golem] ${w.message}"))
  result.files.map { gf =>
    val out = managedRoot / gf.relativePath
    IO.write(out, gf.content)
    out
  }
}.taskValue
```

For new generators:
1. Add the pure generation logic to `sdks/scala/codegen/src/main/scala/golem/codegen/`.
2. Add sbt integration in `sdks/scala/sbt/src/main/scala/golem/sbt/`.
3. Hook into `Compile / sourceGenerators` as a `.taskValue`.
4. Use `FileFunction.cached` with `FileInfo.hash` if the generation has an input file (schema, WIT, etc.) to avoid unnecessary regeneration.

### Mill Plugin (sdks/scala/mill/)

The Mill plugin `GolemAutoRegister` is a trait mixed into `ScalaJSModule`. It uses `generatedSources` and `T { ... }` tasks. Follow the same pattern as `golemGeneratedAutoRegisterSources`.

### Shared logic, not duplicated

All generation logic lives in `sdks/scala/codegen/`. The sbt and Mill plugins are thin wrappers that:
- Collect source files and read their contents
- Call the shared `generate(...)` function
- Log warnings
- Write returned files under managed/generated roots
- Configure build-tool-specific hooks (module initializers, compile dependencies)

When adding a new generation step, implement the logic **once** in `sdks/scala/codegen/`, then add thin wrappers in both `GolemPlugin.scala` and `GolemAutoRegister.scala`.

## Existing Code Generation in Golem Scala SDK

### 1. Auto-Registration (shared codegen + sbt/Mill wrappers)

Scans sources for `@agentImplementation` classes using scalameta's parser, then generates `RegisterAgents.scala` and per-package `__GolemAutoRegister_*.scala` files using scalameta quasiquotes.

**Key behavior:** When `golemBasePackage` is set, the plugin adds a `scalaJSModuleInitializer` pointing to `RegisterAgents.main()`. The codegen generates this class **only** if `@agentImplementation` classes are found. If no implementations are found (e.g. because source directories don't include the component subdirectory), the module initializer references a non-existent class, causing a Scala.js linker error. See the "Known Issue: Multi-Component App" section in the `golem-scala-development` skill.

**Files:**
- `sdks/scala/codegen/src/main/scala/golem/codegen/autoregister/AutoRegisterCodegen.scala` — shared logic
- `sdks/scala/sbt/src/main/scala/golem/sbt/GolemPlugin.scala` — sbt wrapper (source generator + module initializer)
- `sdks/scala/mill/src/golem/mill/GolemAutoRegister.scala` — Mill wrapper

### 2. RPC Client Generation (shared codegen + sbt/Mill wrappers)

Scans sources for `@agentDefinition` traits, extracts their method surfaces, and generates `XClient` companion objects with `XRemote` traits and per-method wrapper classes.

**Generated per-method class provides five call modes:**
- `apply(args...)` — async await via `asyncInvokeAndAwait` host function + pollable
- `cancelable(args...)` — returns `(Future[Out], CancellationToken)` for cancellable async await
- `trigger(args...)` — fire-and-forget via `invoke` host function
- `scheduleAt(args..., when)` — scheduled invocation
- `scheduleCancelableAt(args..., when)` — cancelable scheduled invocation

**Runtime call chain:** Generated method → `AbstractRemoteMethod.awaitWith/cancelableAwaitWith/triggerWith/scheduleWith` → `ResolvedAgent.await/cancelableAwait/trigger/schedule` → `RpcInvoker.asyncInvokeAndAwait/cancelableAsyncInvokeAndAwait/invoke/...` → `WasmRpcApi.WasmRpcClient` → WIT `golem:agent/host@1.5.0` `wasm-rpc` resource

**Key async detail:** The default `apply()` path uses `async-invoke-and-await` (not `invoke-and-await`), returning a `FutureInvokeResult` resource. The runtime polls via `subscribe()` → `pollable.promise()` → `get()`, yielding genuine async `Future`s that allow concurrent RPC calls. This matches the TypeScript SDK behavior.

**Files:**
- `sdks/scala/codegen/src/main/scala/golem/codegen/rpc/RpcCodegen.scala` — shared generation logic
- `sdks/scala/core/js/src/main/scala/golem/runtime/rpc/AbstractRemoteMethod.scala` — base class for generated wrappers
- `sdks/scala/core/js/src/main/scala/golem/runtime/rpc/AgentClientRuntime.scala` — `ResolvedAgent` with async/cancelable dispatch
- `sdks/scala/core/js/src/main/scala/golem/runtime/rpc/RemoteAgentClient.scala` — `WasmRpcInvoker` implementing pollable-based async
- `sdks/scala/core/js/src/main/scala/golem/runtime/rpc/host/WasmRpcApi.scala` — Scala.js facades for `WasmRpc` and `FutureInvokeResult`
- `sdks/scala/core/js/src/main/scala/golem/runtime/rpc/RpcInvoker.scala` — trait with sync, async, and cancelable invoke methods
- `sdks/scala/core/js/src/main/scala/golem/runtime/rpc/CancellationToken.scala` — cancellation token (wraps `() => Unit`)
- `sdks/scala/sbt/src/main/scala/golem/sbt/GolemPlugin.scala` — sbt wrapper
- `sdks/scala/mill/src/golem/mill/GolemAutoRegister.scala` — Mill wrapper

### 3. Scala 3 Macros (compile-time, not build-time)

Macros generate code at compile time, not as a build step. They live in `sdks/scala/macros/` and use `scala.quoted.*`:

- `AgentDefinitionMacro` — extracts `AgentMetadata` from `@agentDefinition` traits
- `AgentImplementationMacro` — generates implementation wrappers from `@agentImplementation` classes
- `AgentClientMacro` — generates RPC client types
- `AgentCompanionMacro` — generates companion object boilerplate (`get`, `getPhantom`, etc.)

These are **not** build-time code generators. Do not confuse them with `sourceGenerators`.

## Generation Pipeline Shape

Follow this pipeline for new generators:

```
1. Load schema/input    (WIT file, annotation scan, external spec)
2. Parse into models    (typed case classes, not raw strings)
3. Classify/transform   (determine what code to emit)
4. Build AST            (scalameta quasiquotes)
5. Pretty-print         (.syntax on the AST root)
6. Return               (GeneratedFile with relativePath + content)
```

The plugin wrappers handle file writing, formatting, and incremental build integration.

## Conventions

- **Pure functions** — all generator methods are pure. No ZIO, no sbt/Mill types, no file I/O in the shared library.
- **Trait mixin composition** — split generators into traits (`ModelGenerator`, `ClientGenerator`, etc.) and mix them into the main codegen class if complexity warrants it.
- **Dialect-aware** — use `dialects.Scala3` for parsing user sources (with `Scala213` fallback). Use `Scala213` for quasiquote construction (compatible with both 2.12 and 3.x codegen host).
- **Generated file header** — include `/** Generated. Do not edit. */` as a comment in generated objects/classes.
- **Output location** — write to `sourceManaged` (sbt) or `T.dest` (Mill), never to source directories.
