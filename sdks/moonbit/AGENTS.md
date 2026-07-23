# Golem SDK for MoonBit — Agent Guide

## Project Overview

This project builds a **Golem SDK for the MoonBit programming language**. Golem is a platform for
building durable, fault-tolerant applications using WebAssembly components. The SDK's purpose is to
let MoonBit developers write Golem agents without dealing with WIT (WebAssembly Interface Types)
directly.

The SDK targets the **new agent schema model**: `golem:core/types@2.0.0` and `golem:agent@2.0.0`.
No live API, code path, or generated stub uses the old value/type carriers (`WitValue`/`WitType`/
`WitNode`/`DataValue`/`DataSchema`/`ElementValue`/`ElementSchema`) — including internals and the
hand-maintained generated stubs. (This document mentions those names only to state that they are
gone.)

### Reference SDKs

The design is modeled after the existing Rust and TypeScript SDKs in
[golemcloud/golem](https://github.com/golemcloud/golem) (`sdks/rust` and `sdks/ts`). These SDKs
share a layered architecture:

1. **WIT bindings layer** — generated code implementing the `golem:agent/guest` WIT world exports
   (`initialize`, `invoke`, `discover-agent-types`, `get-definition`) and
   `golem:api/save-snapshot` / `golem:api/load-snapshot`.
2. **Registry layer** — a global agent registry where agent types are registered at startup;
   the WIT exports delegate to the registry to find and invoke agents.
3. **Serialization layer** — traits that convert between user types and the schema model. In Rust
   this is `IntoValue` / `FromValueAndType`; in MoonBit it is `IntoSchema` / `FromSchema` over the
   recursive `SchemaType` / `SchemaValue` model. In TypeScript, a build-time `typegen` tool
   statically analyzes source to produce schema metadata.
4. **User-facing layer** — proc-macro attributes (`#[agent]`) in Rust, an `@agent()` decorator +
   `BaseAgent` class in TypeScript, or `#derive.agent` annotations + a code-generation tool in
   MoonBit. Users never see WIT.
5. **Durability layer** — a wrapper that checks if execution is in live or replay mode, persisting
   or replaying side-effect results via the oplog.

## Repository Structure

```
sdks/moonbit/
├── AGENTS.md                          # This file
├── release.sh                         # Release/dev mode toggle script
├── golem_sdk/                         # The SDK library (published as golemcloud/golem_sdk)
│   ├── moon.mod.json                  # Module: golemcloud/golem_sdk (preferred-target: wasm)
│   ├── moon.pkg                       # Root package
│   ├── README.mbt.md                  # User-facing readme (also the published readme)
│   ├── scripts/
│   │   └── regen-bindings.sh          # Regenerates WIT bindings with stock wit-bindgen + fixes
│   ├── wit/                           # WIT definitions
│   │   ├── main.wit                   # The `agent-guest` world definition (core/types@2.0.0)
│   │   ├── deps.toml / deps.lock      # WIT dependency sources
│   │   └── deps/                      # Vendored WIT dependencies (golem-core-v2, golem-agent@2.0.0, …)
│   ├── interface/                     # WIT-generated MoonBit types and host-import bindings
│   │   └── golem/
│   │       ├── core/types/            # The schema model WIT types (golem:core/types@2.0.0)
│   │       ├── agent/common/          # AgentType, AgentError, Principal, HTTP details, etc.
│   │       ├── agent/host/            # Host import: get_all_agent_types
│   │       ├── api/                   # host, context, oplog, retry bindings
│   │       ├── durability/durability/ # Durability host imports
│   │       ├── quota/                 # Quota-token host bindings
│   │       ├── rdbms/                 # RDBMS (Postgres, MySQL, …) bindings
│   │       └── websocket/             # WebSocket client bindings
│   │   └── wasi/                      # WASI bindings (blobstore, clocks, io, keyvalue, …)
│   ├── gen/                           # WIT-generated WASM export glue code
│   │   ├── ffi.mbt                    # mbt_ffi_cabi_realloc, return_area, malloc/free/ptr helpers
│   │   ├── world_agent_guest_export.mbt
│   │   └── interface/golem/
│   │       ├── agent/guest/stub.mbt          # SDK impl of initialize/invoke/get_definition/discover
│   │       ├── api/saveSnapshot/stub.mbt      # Snapshot save dispatch
│   │       └── api/loadSnapshot/stub.mbt      # Snapshot load dispatch
│   ├── world/                         # WIT-generated world-level bindings
│   ├── schema_model/                  # The recursive schema model + WIT conversions
│   │   ├── model.mbt                  # SchemaType/SchemaTypeBody, SchemaValue, SchemaGraph, SchemaTypeDef
│   │   ├── builder.mbt                # SchemaBuilder (register/reserve/ref_/commit/build_graph)
│   │   ├── wit.mbt                    # to/from golem:core/types@2.0.0 (GraphEncoder/GraphDecoder, merge)
│   │   ├── validation.mbt            # Structural validation (validate_graph -> [SchemaError])
│   │   ├── errors.mbt                # SchemaError / SchemaModelError
│   │   ├── roundtrip_test.mbt
│   │   └── validation_test.mbt
│   ├── schema/                        # Serialization traits + primitive/compound impls
│   │   ├── schema.mbt                 # IntoSchema / FromSchema traits, TypeTag, helpers
│   │   ├── primitives.mbt            # Impls for String, Bool, Int, UInt, Int64, UInt64, Float, Double, Byte, Char, Unit
│   │   ├── compounds.mbt             # Impls for Option[T], Array[T], Result[T,E], Map[K,V], Bytes
│   │   ├── tuples.mbt                # Impls for tuples (arity 2–8)
│   │   ├── records.mbt              # record/enum/variant helpers used by generated code
│   │   ├── schema_test.mbt
│   │   └── records_test.mbt
│   ├── multimodal/                    # Schema-native multimodal support
│   │   ├── multimodal.mbt            # Multimodal[T] + MultimodalModality trait + IntoSchema/FromSchema
│   │   └── multimodal_test.mbt
│   ├── agents/                        # Agent registry + dispatch runtime
│   │   ├── agents.mbt                # AgentState, RegisteredAgent, RawAgent/Snapshottable traits, register_agent
│   │   ├── agent_type.mbt            # AgentTypeDef/Method/Constructor/Config defs -> @common.AgentType
│   │   ├── dispatch.mbt              # encode/decode invocation input/output via @schema
│   │   └── principal_json.mbt        # Principal/UUID <-> JSON helpers
│   ├── config/                        # Code-first configuration (Config[T], Secret[T])
│   ├── errors/                        # AgentError construction/decoding helpers
│   ├── rpc/                           # Agent-to-agent RPC client helpers (AgentClient)
│   ├── context/                       # Span-based tracing / invocation context
│   ├── logging/                       # Structured logging (named loggers, level filtering)
│   ├── http/                          # HTTP types re-exported from WIT
│   ├── webhook/                       # Webhook helper (create/await incoming POST via promise)
│   ├── api/                           # Golem host API re-exports (agents, idempotency, …)
│   ├── quota/                         # Quota-token helpers
│   ├── filesystem/                    # Filesystem helpers
│   └── ffi/                           # Shared FFI helpers
├── golem_sdk_tools/                   # Code generation tools (native CLI, not WASM)
│   ├── moon.mod.json                  # Module: golemcloud/golem_sdk_tools (deps: moonbitlang/x, /parser, /formatter)
│   ├── lib/                           # Library package
│   │   ├── mbti.mbt                   # Parser for .mbt source files (extracts pub fn signatures)
│   │   ├── reexports.mbt             # AST construction: generates reexport wrapper functions
│   │   ├── agents.mbt                # Agent source parser: #derive.agent structs, constructors, methods
│   │   ├── agents_emit.mbt           # Agent code emitter: registration, RawAgent impls (AST)
│   │   ├── value_types.mbt           # #derive.golem_schema parser (records, enums, variants)
│   │   ├── value_types_emit.mbt      # IntoSchema/FromSchema impl emitter
│   │   ├── multimodal_emit.mbt       # MultimodalModality impl emitter
│   │   ├── config_emit.mbt           # Config declaration emitter
│   │   ├── clients_emit.mbt          # RPC client stub emitter
│   │   ├── http_validation.mbt       # HTTP mount/endpoint validation (Principal rules, path vars)
│   │   ├── ast_helpers.mbt           # AST construction helpers
│   │   ├── pkg.mbt                   # moon.pkg parser/updater (link section, imports)
│   │   └── *_test.mbt                # Tests for parsing, emission, validation
│   └── cmd/
│       └── main.mbt                   # `reexports` and `agents` subcommands
└── golem_sdk_example1/                # Example consumer project (also the user template)
    ├── moon.mod.json                  # Module: golemcloud/golem_sdk_example1 (deps on local golem_sdk)
    ├── golem.yaml                     # Golem application manifest with the codegen+build pipeline
    ├── README.mbt.md
    ├── wit/                           # Example world (core/types@2.0.0, agent/guest@2.0.0)
    └── golem_moonbit_examples/        # The component package (8 example agents)
        ├── moon.pkg                   # is-main, WASM export link config (auto-managed by tools)
        ├── counter.mbt               # Counter (state, snapshotting, fn main {})
        ├── task_manager.mbt          # TaskManager + Priority/TaskInfo (#derive.golem_schema)
        ├── config_agent.mbt          # ConfiguredAgent + #derive.config + @config.Secret
        ├── http_agent.mbt            # WeatherAgent (HTTP mount + endpoints)
        ├── multimodal_agent.mbt      # VisionAgent + TextOrImage (#derive.multimodal)
        ├── audit_agent.mbt           # AuditLog (constructor Principal injection)
        ├── rpc_example.mbt           # RpcExampleAgent (calls generated client stubs)
        ├── webhook_agent.mbt         # WebhookAgent (@logging + @webhook)
        ├── golem_reexports.mbt       # Generated — re-exports WASM entry points from SDK gen package
        ├── golem_agents.mbt          # Generated — agent registration, RawAgent dispatch, init block
        ├── golem_derive.mbt          # Generated — IntoSchema/FromSchema (+ multimodal) impls
        └── golem_clients.mbt         # Generated — RPC client stubs
```

## Key Concepts

### WIT Bindgen

All code under `interface/`, `world/`, and `gen/` (except the `gen/interface/*/stub.mbt` files) is
**auto-generated** by `wit-bindgen moonbit`. Do NOT edit these files. Regenerate with the script,
which uses **stock `wit-bindgen` (no fork)** and applies the required post-processing:

```sh
cd golem_sdk
moon run script bindgen          # or: bash scripts/regen-bindings.sh
```

The script (`scripts/regen-bindings.sh`):
1. Runs `wit-bindgen moonbit ./wit --derive-show --derive-eq --derive-error --project-name golemcloud/golem_sdk --ignore-stub`.
2. Fixes a stock-bindgen `s8`/`s16` double sign-extension bug (the generated code does a signed
   load *and* subtracts `0x100`/`0x10000`; the spurious subtraction is stripped).
3. Removes the `moon.pkg.json` files wit-bindgen emits (this repo tracks hand-maintained plain
   `moon.pkg` files; keeping both makes `moon` warn).
4. Asserts the s8/s16 fix took effect.

`--ignore-stub` means wit-bindgen will NOT (re)generate the stub files. The `stub.mbt` files under
`gen/interface/` are the **SDK's implementation** of the WIT export interfaces — the dispatch logic
the SDK actually runs. They are maintained by hand and operate purely on the new schema carrier
(`@types.SchemaValueTree`); they contain no legacy value/type types.

### The Agent Registry Pattern

The core pattern (in `agents/agents.mbt`):

1. A global `AgentState` holds a registry of `RegisteredAgent`s and a single current
   `AgentInstance?`.
2. Each agent type calls `register_agent(...)` during the generated `fn init {}` to register itself.
3. When the host calls `initialize(agent_type, input)`, the SDK looks up the registered agent by
   name, calls its `construct` function with the decoded constructor input + `Principal`, and stores
   the resulting `AgentInstance`.
4. When the host calls `invoke(method_name, input)`, the SDK delegates to the instance's
   `RawAgent::invoke`.
5. `discover_agent_types()` returns all registered agent types.
6. `get_definition()` returns the current instance's type.

### The RawAgent Trait

```moonbit
pub(open) trait RawAgent {
  invoke(
    Self,
    method_name : String,
    input : @types.SchemaValueTree,
    principal : @common.Principal,
  ) -> Result[@types.SchemaValueTree?, @common.AgentError]
}
```

This is the low-level interface every agent implements. The result is `Ok(None)` for `Unit`/no-return
methods and `Ok(Some(tree))` otherwise (the guest stub does NOT normalize `Some(empty-tuple)` to
`None` — the generated dispatcher has the method-schema context and decides). The `agents` code
generation tool auto-generates `RawAgent` impls with method dispatch, constructor deserialization,
and result serialization. Optional snapshot support is via the `Snapshottable` trait.

### The Schema Model (`schema_model`)

The `schema_model` package is the SDK's recursive, in-memory representation of the new
`golem:core/types@2.0.0` model. It replaces the old WitValue/WitType node-tree carriers entirely.

| Type | Purpose |
|---|---|
| `SchemaType { body, metadata }` | A type node; `body` is a `SchemaTypeBody` |
| `SchemaTypeBody` | The type variants: primitives, `Record`, `Variant`, `Enum`, `Flags`, `Tuple`, `List`, `FixedList`, `Map`, `Option`, `Result`, `Text`, `Binary`, `Path`, `Url`, `Datetime`, `Duration`, `Quantity`, `Union`, `Secret`, `QuotaToken`, `Future`, `Stream`, and `Ref(id)` |
| `SchemaValue` | The runtime value variants, mirroring the type variants |
| `SchemaGraph { defs, root }` | A type plus its named definitions (`SchemaTypeDef { id, name, body }`); supports recursion via `Ref(id)` |
| `TypedSchemaValue { graph, value }` | A self-describing value (graph + value) |
| `SchemaBuilder` | Registers named type defs (with `reserve`/`commit` so recursive/self-referential types close to `Ref(id)`); `build_graph` finalizes a `SchemaGraph` |

WIT conversion (`wit.mbt`): `schema_graph_to_wit` / `schema_graph_from_wit`,
`schema_value_to_wit` / `schema_value_from_wit`, `typed_schema_value_to_wit` / `…_from_wit`, and
`merge_agent_graphs` (the equivalent of the Rust `conversion.rs` merge). The wire types come from
`interface/golem/core/types` (`@types.SchemaGraph`, `@types.SchemaValueTree`,
`@types.TypedSchemaValue`, etc.). Structural validation lives in `validation.mbt`
(`validate_graph -> Array[SchemaError]`).

### Serialization Traits (`schema`)

The `schema` package is the SDK's equivalent of the Rust SDK's `IntoValue` / `FromValueAndType`:

```moonbit
pub(open) trait IntoSchema {
  type_id() -> String
  register_in(@schema_model.SchemaBuilder) -> @schema_model.SchemaType
  to_value(Self) -> @schema_model.SchemaValue
}

pub(open) trait FromSchema {
  from_value(@schema_model.SchemaValue) -> Self raise FromSchemaError
}
```

- `type_id()` defaults to the package-qualified MoonBit type name (bare name at module root); a
  `named` override is reserved for intentional cross-SDK identity. Exact cross-SDK string identity is
  *not* required.
- Implemented for: `Unit`, `String`, `Bool`, `Int` (S32), `UInt` (U32), `Int64` (S64), `UInt64`
  (U64), `Float` (F32), `Double` (F64), `Byte` (U8), `Char`, `Bytes`, `Option[T]`, `Array[T]`,
  `Result[T, E]`, `Map[K, V]`, and tuples of arity 2–8.

**Helpers** (in `schema.mbt`):
- `type_tag[T]() -> TypeTag[T]` — a zero-sized handle to ask for a type's schema without a value
- `schema_graph_of_tag(TypeTag[T])` / `into_schema_graph(TypeTag[T])` — build a `SchemaGraph` for `T`
- `register_in_with(SchemaBuilder, TypeTag[T])` — register `T` into a shared builder
- `to_value_as[T](v)` / `from_value_as[T](SchemaValue)` — typed (de)serialization
- `try_into_typed_schema_value[T](v)` — produce a self-describing `TypedSchemaValue`
- `record_field`, `expect_record`, `expect_variant`, `expect_enum`, `check_case_index`,
  `value_kind`, … — building blocks used by generated `golem_derive.mbt`
- `FromSchemaError` — `ShapeMismatch` / `OutOfRange` / `UnknownUnionTag` / `Custom`

### Multimodal (`multimodal`)

Multimodal input/output is schema-native — there is no resurrected `DataValue`/`ElementValue`:

```moonbit
pub(all) struct Multimodal[T] { items : Array[T] }

pub(open) trait MultimodalModality {
  multimodal_type_id() -> String
  multimodal_cases(@schema_model.SchemaBuilder) -> Array[@schema_model.VariantCaseType]
  to_modality_value(Self) -> (UInt, @schema_model.SchemaValue)
  from_modality_value(UInt, @schema_model.SchemaValue) -> Self raise @schema.FromSchemaError
}
```

`Multimodal[T]` implements `IntoSchema`/`FromSchema` when `T : MultimodalModality`. The schema is
`List(Variant(...))` where the list root's `metadata.role = @types.Role::Multimodal`; the value is a
list of `Variant(case_idx, Some(payload))`. Nesting a multimodal type inside
`Option`/`Array`/`Result`/`Tuple`/derived fields is rejected by the code generator
(`value_types.mbt` / `http_validation.mbt`).

### Code Generation (`golem_sdk_tools`)

The `golem_sdk_tools` CLI automates the boilerplate that connects user agent definitions to the
runtime. It parses `.mbt` source with `moonbitlang/parser`, constructs AST nodes, and emits MoonBit
source via `moonbitlang/formatter`. Two subcommands:

#### `reexports` subcommand

```sh
cd golem_sdk_tools
moon run cmd -- reexports <sdk-path> <target-dir>
# e.g.: moon run cmd -- reexports ../golem_sdk ../golem_sdk_example1/golem_moonbit_examples
```

Generates `golem_reexports.mbt` (re-exports the WASM entry points — `cabi_realloc`, `wasmExport*` —
from the SDK's `gen` package) and updates the target `moon.pkg`: it ensures the
`golemcloud/golem_sdk/gen` import (`@gen`) is present and rewrites the `link.wasm.exports` section.
(The target `moon.pkg` must use a multi-line `options(` block so the link section can be inserted.)

#### `agents` subcommand

```sh
cd golem_sdk_tools
moon run cmd -- agents <package-dir>
# e.g.: moon run cmd -- agents ../golem_sdk_example1
```

Generates, from source annotations:

1. **`golem_agents.mbt`** — `fn init {}` registration, `AgentTypeDef` definitions (schemas derived
   from method signatures via `@schema`), constructor decoding, and `impl RawAgent` with method
   dispatch. Parameter/result (de)serialization goes through `@schema` and the
   `@types.SchemaValueTree` carrier.
2. **`golem_derive.mbt`** — `IntoSchema` / `FromSchema` impls for `#derive.golem_schema` types, and
   `MultimodalModality` impls for `#derive.multimodal` enums.
3. **`golem_clients.mbt`** — RPC client stubs for agent-to-agent calls.

**Source annotations recognized:**
- `#derive.agent` on a struct — marks it as a Golem agent (`#derive.agent("ephemeral")` for
  ephemeral mode; default is durable).
- `#derive.golem_schema` on a struct or enum — generates `IntoSchema`/`FromSchema` impls.
- `#derive.multimodal` on an enum — generates a `MultimodalModality` impl.
- `#derive.config` — code-first configuration declarations.
- `#derive.prompt_hint("...")` on methods — adds a prompt hint.
- HTTP mount/endpoint annotations — drive the agent's HTTP mount + endpoint schema.
- Doc comments (`///`) on structs, constructors, and methods become descriptions.

There are **no** `#derive.text_languages` / `#derive.mime_types` annotations — the old unstructured
text/binary annotation machinery was removed (see "Parity gap" below).

**Type parsing** (`agents.mbt`) recognizes `Simple(name)`, `Optional(T)`, `List(T)`,
`ResultType(T, E)`, `Tuple(elems)`, `@multimodal.Multimodal[T]`, and `Parameterized(name, params)`.
The emitters build `@syntax.Impl` AST nodes via `ast_helpers.mbt` and serialize them with
`@formatter`.

### User-Facing API

Users write agents with minimal boilerplate:

```moonbit
/// Counter agent in MoonBit
#derive.agent
struct Counter {
  name : String
  mut value : UInt64
}

/// Creates a new counter with the given name
fn Counter::new(name : String) -> Counter {
  { name, value: 0 }
}

/// Increments the counter
pub fn Counter::increment(self : Self) -> Unit {
  self.value += 1
}

/// Returns the current value of the counter
pub fn Counter::get_value(self : Self) -> UInt64 {
  self.value
}
```

Custom data types used in method parameters/return types need `#derive.golem_schema`:

```moonbit
#derive.golem_schema
pub(all) enum Priority {
  Low
  Medium
  High
} derive(Eq)

#derive.golem_schema
pub(all) struct TaskInfo {
  title : String
  priority : Priority
  description : String?
}
```

An empty `fn main {}` must exist in the main package. Multiple agents can coexist in one package —
each is registered in the single generated `fn init {}`.

### Durability

The durability system (host imports in `interface/golem/durability/durability/`) provides
`current_durable_execution_state()`, `begin_durable_function`, `end_durable_function`,
`persist_durable_function_invocation` (+ typed variant), and `read_persisted_durable_function_invocation`
(+ typed variant). The pattern for wrapping a side-effecting call:
1. `begin_durable_function` to get the oplog position.
2. Check `is_live` from `current_durable_execution_state()`.
3. If live: execute the real operation, then `persist_*` the result.
4. If replaying: `read_persisted_*` to get the cached result.
5. `end_durable_function`.

## Current State & What Works

- WIT bindings are fully generated against `golem:core/types@2.0.0` + `golem:agent@2.0.0` and compile
  for the `wasm` target (stock wit-bindgen + post-processing script).
- The agent registry + dispatch pattern is implemented over the new `SchemaValueTree` carrier.
- The schema model (`schema_model`) and serialization traits (`schema`) are complete: `IntoSchema` /
  `FromSchema` for all primitives and compounds (`Option`, `Array`, `Result`, `Map`, `Bytes`, tuples
  2–8), recursive/self-referential custom types, plus record/enum/variant helpers for generated code.
- Multimodal is schema-native (`multimodal` package: `Multimodal[T]` + `MultimodalModality`,
  `list<variant>` with `role = multimodal`).
- Code generation is complete: `reexports` (→ `golem_reexports.mbt` + `moon.pkg`) and `agents`
  (→ `golem_agents.mbt`, `golem_derive.mbt`, `golem_clients.mbt`).
- The example package `golem_sdk_example1/golem_moonbit_examples` (8 agents) builds end-to-end:
  codegen → `moon build --target wasm --release` → `wasm-tools component embed`/`new`, and the final
  component validates with `import golem:core/types@2.0.0` / `export golem:agent/guest@2.0.0`.
- Agent mode (durable/ephemeral), prompt hints, code-first config (`Config[T]`/`Secret[T]`), HTTP
  mounts, and agent-to-agent RPC are supported.
- **No legacy carriers anywhere** — `WitValue`/`WitType`/`WitNode`/`WitTypeNode`/`DataValue`/
  `DataSchema`/`ElementValue`/`ElementSchema` and the deleted `builder`/`extractor`/`agents/types`
  packages are gone from the SDK, the tools, the generated stubs, and the example.

### Parity gap (tracked follow-up, not part of the carrier migration)

The new model has first-class `Text`/`Binary`/`Url` type/value nodes (in `schema_model`), but the
SDK does **not** yet ship a high-level *user-facing* wrapper equivalent to the old
`UnstructuredText`/`UnstructuredBinary` types (Rust's `unstructured_text.rs`/`unstructured_binary.rs`,
TS's `textInput.ts`/`binaryInput.ts`). String maps to `SchemaTypeBody::String` (not `Text`) and
`Bytes` maps to inline `Binary`. Adding ergonomic Text/Binary/Url wrappers (with language/MIME
restrictions and inline-or-URL semantics) is a deliberate **feature addition** for a later phase —
it must be designed against the new schema shape, and must NOT reintroduce the deleted annotation
machinery (`#derive.text_languages` / `#derive.mime_types`) or the old carriers.

## Build & Test Commands

```sh
# In golem_sdk/ (the library, WASM target):
moon check --target wasm          # Type-check
moon build --target wasm          # Build
moon test --target wasm           # Run tests
moon info && moon fmt             # Regenerate .mbti and format
moon run script bindgen           # Regenerate WIT bindings (stock wit-bindgen + fixes)

# In golem_sdk_tools/ (the codegen CLI, native target):
moon check
moon test
moon info && moon fmt
moon run cmd -- reexports <sdk-path> <target-dir>
moon run cmd -- agents <package-dir>

# In golem_sdk_example1/ (the example/template):
moon check --target wasm
moon build --target wasm --release
# Component link (release):
wasm-tools component embed wit \
  _build/wasm/release/build/golem_moonbit_examples/golem_moonbit_examples.wasm \
  --encoding utf16 --output _build/wasm/release/golem_moonbit_examples.embed.wasm
wasm-tools component new \
  _build/wasm/release/golem_moonbit_examples.embed.wasm \
  --output _build/wasm/release/golem_moonbit_examples.agent.wasm
wasm-tools validate _build/wasm/release/golem_moonbit_examples.agent.wasm
```

The example's `golem.yaml` drives the same pipeline (codegen → `moon build` → `wasm-tools`) under
`golem build`. The final component WASM is at
`golem_sdk_example1/_build/wasm/release/golem_moonbit_examples.agent.wasm`.

## Release Script

`sdks/moonbit/release.sh` toggles `golem_sdk_example1` between **development mode** (local path deps,
relative tool paths) and **release/template mode** (versioned mooncakes deps; tools run from
`.mooncakes/`).

```sh
./release.sh 0.1.0              # same version for SDK and tools
./release.sh 0.1.0 0.2.0        # different versions for SDK and tools
./release.sh --dev             # revert to local path deps for in-repo development
```

Release mode rewrites the example's `moon.mod.json` (`deps` path → versioned; adds `bin-deps` with
`golemcloud/golem_sdk_tools`) and the tool/SDK paths in `golem.yaml`, producing a standalone template
that depends only on mooncakes. Both `golemcloud/golem_sdk` and `golemcloud/golem_sdk_tools` must be
published to mooncakes.io for the release template to work.

## Coding Conventions

- MoonBit blocks are separated by `///|` — order is irrelevant.
- `snake_case` for functions/values, `UpperCamelCase` for types/enums.
- Files generated by `wit-bindgen` are marked `// Generated by wit-bindgen ... DO NOT EDIT!`.
- Files generated by `golem_sdk_tools` are marked `// Generated by golem_sdk_tools — DO NOT EDIT!`.
- SDK stub files (`gen/interface/*/stub.mbt`) ARE maintained by hand despite living in `gen/`.
- The SDK library targets WASM only — use `moon check --target wasm` frequently.
- Prefer `inspect()` snapshot tests (`moon test --update`); use `assert_eq` for structural checks.
- Run `moon info && moon fmt` before finalizing changes.

## Important Technical Notes

- The SDK library targets **WASM only** (`preferred-target: wasm` in `moon.mod.json`).
- String encoding is **UTF-16** (passed to `wasm-tools component embed --encoding utf16`).
- Memory management uses `mbt_ffi_malloc`/`mbt_ffi_free` for WASM linear memory; MoonBit's GC manages
  MoonBit objects. `mbt_ffi_cabi_realloc` in `gen/ffi.mbt` is the Component Model canonical ABI
  allocator.
- The `agents` package holds module-level mutable state (a singleton `AgentState`).
- WASM exports are linked via `moon.pkg` `link` config — auto-generated by the `reexports` tool.
- Generated files (`golem_reexports.mbt`, `golem_agents.mbt`, `golem_derive.mbt`, `golem_clients.mbt`)
  must not be edited by hand.
- Multiple agents can coexist in one package — all registered in the single generated `fn init {}`.

## Dependencies & Tools

- **wit-bindgen** — stock `wit-bindgen` with the `moonbit` backend (no fork; validated against
  `wit-bindgen-cli` 0.57.x). Bindings are regenerated via `scripts/regen-bindings.sh`, which applies
  the s8/s16 sign-extension fix in post-processing.
- **wasm-tools** — `component embed` (adds WIT type info) and `component new` (creates the Component
  Model WASM).
- **moon** — MoonBit build tool.
- **moonbitlang/parser** (0.2.5) — source parsing + AST construction for `golem_sdk_tools`.
- **moonbitlang/formatter** (0.1.5) — emitting generated MoonBit source from AST.
- **moonbitlang/x** (0.4.39) — filesystem + env args for `golem_sdk_tools`.
