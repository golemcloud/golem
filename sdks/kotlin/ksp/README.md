# golem-kotlin-ksp

The compile-time half of the [Golem Kotlin SDK](../README.md). It is a
[KSP](https://kotlinlang.org/docs/ksp-overview.html) (Kotlin Symbol Processing) processor that reads
the annotations in your agent source (`@Agent`, `@Endpoint`, `@Tool`, `@RemoteAgent`, …) and generates
the Kotlin that turns an ordinary class into a Golem component.

Maven coordinates: `cloud.golem:golem-kotlin-ksp`. It is a JVM artifact that runs inside the Kotlin
compiler. It is never packaged into the Wasm component.

---

## Why this module is required

A Golem component is a WebAssembly Component. The Golem host talks to it only through the exports
declared by the `golem:agent/guest@2.0.0` WIT interface (`initialize`, `invoke`, `get-definition`,
`discover-agent-types`, …) and, for tools, `golem:tool/*`. The host also needs a full schema of every
agent: its constructor, its methods, the type of each parameter and result, and its HTTP bindings.

A Kotlin agent is just a class with annotations:

```kotlin
@Agent(mount = "/counters/{name}")
class CounterAgent(val name: String) : BaseAgent() {
    @Endpoint(post = "/increment")
    fun increment(): Int { ... }
}
```

Nothing in that source exports a Wasm function or describes a schema. Something has to bridge the
two, and the bridge cannot be written by hand or by reflection:

1. **Kotlin/Wasm has no runtime reflection.** The SDK cannot look at `CounterAgent` at run time to
   find its methods, parameter types or annotations. The information must be extracted at build time.
2. **`@WasmExport` must be declared in the agent's own module.** Kotlin/Wasm dead-code-eliminates
   unreferenced code. It also drops the `@WasmExport` functions of a library dependency, and it does
   not run top-level initialisers when a reactor component is instantiated. Agent registration
   therefore needs a trigger that is guaranteed to be reachable. The generated `@WasmExport` entry
   points are that trigger: the host must call them to use the agent at all, so registration is
   guaranteed to run first.
3. **The wire format is a dynamic value tree.** Arguments and results cross the boundary as
   `SchemaValue`s, and the host expects a precise schema for each type. A generated, statically typed
   converter for each type is smaller and faster than a runtime one, and it needs no reflection.
4. **Mistakes should fail the build, not the deploy.** Route templates, type support and tool
   definitions are checked here, so a bad agent is rejected by `gradle build` with an error that points
   at the offending declaration.

KSP is the right tool because it sees the full Kotlin type model (data classes, sealed hierarchies,
nullability, generics, inherited annotations) and runs as part of the normal Kotlin/Wasm compile.

---

## What it does

For the annotated sources in a module, the processor:

| Input | Generated output |
|---|---|
| `@Agent` class | `<Class>Registration.kt` with `register<Class>()`, which builds a `NativeAgentDescriptor` (type name, mount, constructor and method schemas, HTTP endpoints, auth, CORS, mode, snapshotting, config) plus the factory and handler lambdas that decode `SchemaValue`s and call your code |
| `@HttpRouter` class | The same registration, for a router agent (static files, response headers, OpenAPI provider) |
| Any of the above, `@Tool`, `@ToolMiddleware`, `@ToolImplementation` | `GolemGeneratedGuest.kt`: the single entry point that declares the real `@WasmExport` functions (`golem:agent/guest@2.0.0`, `golem:tool/guest`, `golem:tool/tool-middleware-guest`, `golem:api/save-snapshot` / `load-snapshot`, `cabi_realloc` and the matching `cabi_post_*` functions) and calls every `register…()` before any SDK logic runs |
| `@RemoteAgent` interface | `<Iface>Rpc`: a typed RPC client that encodes arguments, calls the remote agent through `WasmRpc`, and decodes the result |
| `@ToolDefinition` interface `X` | `XDefinition` (tool record and error codecs), `XClient` (typed calls), `XUnderlying` (the handle a middleware receives), and the `XMiddleware` / `XAdapter<U>` base classes. All are public, so a library can declare the interface and another module can use it |
| `@ToolMiddleware` subclass of a generated base | `<Class>Dispatch`, a `UniversalToolMiddleware` that decodes the command input, calls the typed handler and encodes the result |
| `@ToolImplementation` class | `golemProvide<Class>()`, a `NativeToolProvider` that routes a command path to the implementation |
| `@Agent` class | `<component>-agent.wit`: an informational WIT world and agent metadata. The build does not consume it |

It also **validates**, reporting through `KSPLogger.error` so the build fails:

- HTTP mount paths and `@Endpoint` templates against constructor and method parameters (path
  variables, `{+rest}` catch-alls, query parameters, `@Header` bindings).
- Router and live-file rules, and `staticBindings` / `exposeFiles` mappings, against the shared corpus
  `golem-service-base/tests/fixtures/http-handlers/corpus.json`.
- `@ToolDefinition` shape: kebab-case names, `@Arg` kinds, constraints and refinements, group
  nesting, no clash with generated names.
- Typed middleware: the presented and wrapped tool, a no-argument constructor, no type parameters.
- Duplicate tool, middleware and provider names, `@ReadOnly` on an ephemeral agent, and unsupported
  types (`UnsupportedTypeException`).
- Agents and tools that do not share one package. This is a current limit of the entry point.

---

## How it does it

The processor has three stages. It collects while KSP rounds run and emits in `finish()`.

```
 your sources                 process(resolver)                          finish()
┌──────────────┐   ┌───────────────────────────────────────┐   ┌────────────────────────────┐
│ @Agent       │   │ validate() each symbol (defer if its  │   │ cross-checks (duplicates,  │
│ @Tool        │──▶│ types are not resolved yet)           │──▶│ single package)            │
│ @RemoteAgent │   │ build a model: AgentModel, ToolModel, │   │ NativeRegistrationEmitter  │
│ @ToolDefn …  │   │ ToolDefinitionModel, …                │   │   .emitEntryPoint(...)     │
└──────────────┘   │ validate the model                    │   └────────────────────────────┘
                   │ emit per-symbol files immediately     │
                   └───────────────────────────────────────┘
```

1. **Collect (`GolemAgentProcessor.process`).** For each annotation the processor calls
   `getSymbolsWithAnnotation` and splits the result with `validate()`. A symbol whose types another
   processor has not generated yet is deferred to the next round. The processor reads annotation
   arguments, including those inherited through an override chain, and builds an immutable *model*.
2. **Resolve types (`TypeMapper` → `TypeDesc`).** Every Kotlin type on the agent surface becomes a
   `TypeDesc`. This covers primitives (each at its own width), `Unit`, nullable (`option`), `List`,
   `Map`, `Pair` / `Triple` (`tuple`), enums, sealed classes (`variant`), data classes (`record`),
   `Datetime`, `Either` (`result`), and the unstructured text and binary types. `TypeDesc` is the
   single source of truth. `toWit()` renders the type string the runtime schema builder consumes, and
   `ConverterCodegen` renders the Kotlin that converts to and from `SchemaValue`.
3. **Emit.** Each emitter is a plain `class` that builds a source string and writes it through
   KSP's `CodeGenerator`. Per-symbol outputs (RPC clients, typed tool surfaces, registrations) are
   written during `process`. The `@WasmExport` entry point is written once, in `finish()`, because it
   must see every agent, tool and middleware in the module.

Registration is explicit rather than discovered. `GolemGeneratedGuest.kt` calls
`registerAllAgents()` before `initialize`, `invoke` and the other exports touch the SDK, so the agent
registry is always populated.

Generated files carry the header `// AUTO-GENERATED by golem-kotlin-ksp — do not edit`. They land in
the Gradle KSP output directory, not in your source tree.

---

## Architecture

All sources live in `src/main/kotlin/cloud/golem/ksp/`. The service file
`META-INF/services/com.google.devtools.ksp.processing.SymbolProcessorProvider` registers
`GolemAgentProcessorProvider`, which is how KSP finds the processor.

| Layer | Files | Role |
|---|---|---|
| **Entry point** | `GolemAgentProcessorProvider`, `GolemAgentProcessor` | KSP provider and the processor. It owns the collect, validate and emit sequence and the `finish()` cross-checks |
| **Models** | `AgentModel`, `ToolDefinitionModel`, `ToolArguments` | Plain data classes (`AgentModel`, `MethodModel`, `ParamModel`, `ToolModel`, `ToolDefinitionModel`, `ToolArgModel`, …) that decouple KSP symbols from code generation. Emitters never see a `KSDeclaration` |
| **Type system** | `TypeDesc`, `TypeMapper`, `UnsupportedTypeException` | Kotlin `KSType` → `TypeDesc` → WIT type string |
| **Converters** | `ConverterCodegen`, `ConfigWalker` | Generate `SchemaValue` ↔ Kotlin expressions. `ConfigWalker` flattens a `Config<T>` data-class tree into config declarations, with `Secret<T>` leaves |
| **Emitters** | `NativeRegistrationEmitter`, `RemoteAgentEmitter`, `ToolDefinitionEmitter`, `ToolProviderEmitter`, `TypedMiddlewareEmitter`, `WitEmitter` | Write the generated source files |
| **Validation** | `HttpValidation`, `RouterValidation`, `FileMappings`, `TypedMiddlewareResolver` | Build-time rules. They are ports of the Scala SDK's, so the SDKs agree. `FileMappings` and `RouterValidation` are tested against the shared corpus |
| **Helpers** | `Naming` | Kebab-case names, rendering a `KSType` back to Kotlin source, and KDoc text as a description fallback |

Dependencies point one way: `GolemAgentProcessor` → models and validation → `TypeMapper` →
`TypeDesc`; emitters read models and `TypeDesc` only.

### How it relates to the other modules

```
 sdk/          annotations (cloud.golem.annotations.*) + runtime (cloud.golem.runtime.*)
   ▲                 ▲                             ▲
   │ compile         │ read by name (FQN strings)  │ referenced by generated code
 your agent ──ksp──▶ ksp/ ──────────────────────────┘
   │
   └─ gradle-plugin/  nativeComponent: compile (runs KSP) → wasm-tools embed → new --adapt → validate
```

- **The processor does not depend on the SDK.** It finds annotations by fully qualified name
  (`cloud.golem.annotations.Agent`, …) as strings and emits references to `cloud.golem.runtime.*`
  types as text. Its only dependency is the KSP API. This keeps the JVM processor free of Kotlin/Wasm
  artifacts, but the names are a contract. Rename an annotation in `sdk/` and the processor stops
  seeing it.
- **The tests share that contract.** `KspTestSupport` compiles the real annotation sources from
  `../sdk/src/commonMain/kotlin/cloud/golem/annotations` instead of hand-written stubs, so a drift
  between the two modules fails a test.
- **The Gradle plugin does not configure KSP.** The agent project applies the KSP plugin and adds
  the processor itself. The `cloud.golem.wasm-component` plugin then runs KSP as part of the Wasm
  compile.

---

## Using it

In an agent project (see `../example/build.gradle.kts`):

```kotlin
plugins {
    kotlin("multiplatform") version "2.4.0"
    id("com.google.devtools.ksp") version "2.3.9"
    id("cloud.golem.wasm-component") version "0.1.0"
}

dependencies {
    add("kspWasmWasi", "cloud.golem:golem-kotlin-ksp:0.1.0")
}
```

`golem new --template kotlin` sets this up for you. The SDK, this processor and the Gradle plugin are
versioned together; see [supported versions](../docs/supported-versions.md).

---

## Developing

Requirements: JDK 17+. Run these from this directory.

```bash
./gradlew test                              # processor tests (kctfork compiles real sources through KSP)
./gradlew ktlintCheck detektMain detektTest # style and the four chosen detekt rules
./gradlew publishToMavenLocal               # publish 0.1.0 to ~/.m2 for the example and the CLI template
./gradlew publishToMavenLocal -PgolemSdkVersion=0.0.0-SNAPSHOT   # CI-style snapshot
```

Run `detektMain` and `detektTest`, not the aggregate `detekt` task. The rule
`UnsafeCallOnNullableType` needs type resolution, and only the typed tasks have it. Each source set
keeps its own `detekt-baseline-*.xml`.

### Tests

Tests live in `src/test/kotlin/cloud/golem/ksp/` and use
[kotlin-compile-testing (kctfork)](https://github.com/ZacSweers/kotlin-compile-testing) to run the
processor over real Kotlin sources and assert on the generated files and the logged errors.

| Area | Tests |
|---|---|
| Agent processing and registration | `GolemAgentProcessorTest` |
| Type mapping and converters | `TypeMapperTest`, `ConverterCodegenTest` |
| HTTP and router validation | `HttpValidationTest`, `HttpCorpusTest` |
| Tool definitions and arguments | `ToolDefinitionModelTest`, `ToolDefinitionEmitterTest`, `ToolArgumentModelTest`, `ToolArgumentEmitterTest`, `ToolStreamEmitterTest`, `TypedToolNamesTest` |
| Tool providers and middleware | `ToolProviderEmitterTest`, `TypedMiddlewareEmitterTest` |

Beware of tests that pass without proving anything. KSP binds an unknown named annotation argument to
the first declared parameter, and a test that only checks "compilation succeeded" can pass on empty
output. Assert on the content of the generated file.

### Generated code refers to the SDK by name

The emitters write Kotlin source as strings. Those strings name SDK classes, functions and packages
that the compiler cannot check when the processor is built. The compiler checks them only when it
compiles the generated source in a user project. If a name in the SDK changes, the processor still
builds and its tests may still pass, but every project that uses it fails to compile.

Places that hold such names:

- The `import` lines of `NativeRegistrationEmitter`: `NativeAgentRuntime`, `NativeAgentDescriptor`,
  `NativeMethodDescriptor`, `NativeParamSchema`, `NativeHttpEndpoint` and `SchemaValue`, all in
  `cloud.golem.runtime`.
- The fully qualified names inside the strings of `NativeRegistrationEmitter`, for example
  `cloud.golem.runtime.NativeToolDescriptor`, `NativeDurableStreams`, `NativeFileMapping`,
  `NativeConfigDeclaration`, `cloud.golem.config.Config`, `cloud.golem.MultipartSnapshot` and
  `cloud.golem.wasm.configureArenaHeadroom`.
- `RemoteAgentEmitter`: the imports of `SchemaValue`, `WasmRpc`, `RpcResult` and `RpcException`, and the
  `cloud.golem.config` and `cloud.golem.permissions` names.
- `ConverterCodegen`: `SchemaCodecs`, `SchemaDecodeException`, `Either`, `host.QuotaToken` and
  `SchemaValueBytes`.
- `ToolDefinitionEmitter`, `ToolProviderEmitter` and `TypedMiddlewareEmitter`: the `R` constant
  (`cloud.golem.runtime`) and the imports built from it, such as `callTyped`,
  `callTypedWithOutputs`, `noDeclaredErrorEncoder` and `noDeclaredErrorDecoder`.

When you rename or move an SDK type, or change a signature that generated code calls, search `ksp/src/main`
for the old name and for the string form of its package. Most tests in `GolemAgentProcessorTest` match the text of the generated source only. The one test that
compiles it uses a hand-written JVM stub of `cloud.golem.runtime`, not the real SDK, so a rename in the
SDK does not fail it until you update the stub. The real check is
[`../scripts/real-sdk-compile`](../scripts/README.md#real-sdk-compile), which compiles the generated source of the
acceptance fixtures for Kotlin/Wasm against the published SDK. Run it after every change to an emitter or to the
SDK surface that the emitters name. CI runs it in `build-golem-kotlin`.

### Adding support for a new annotation or type

1. Add the annotation to `sdk/src/commonMain/kotlin/cloud/golem/annotations/`.
2. Read it in `GolemAgentProcessor` and carry it on a model in `AgentModel.kt` or
   `ToolDefinitionModel.kt`.
3. For a new type, add a `TypeDesc` case, then teach `TypeMapper` (resolve), `TypeDesc.toWit()` and
   `ConverterCodegen` (decode and encode). Add the matching runtime support in `sdk/`.
4. Emit it from the right emitter, and add a build-time check where a mistake is possible.
5. Add a `kctfork` test that asserts on the generated source.
