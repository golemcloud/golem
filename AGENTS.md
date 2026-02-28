# Golem SDK for MoonBit — Agent Guide

## Project Overview

This project builds a **Golem SDK for the MoonBit programming language**. Golem is a platform for
building durable, fault-tolerant applications using WebAssembly components. The SDK's purpose is to
let MoonBit developers write Golem agents without dealing with WIT (WebAssembly Interface Types)
directly.

### Reference SDKs

The design is modeled after the existing Rust and TypeScript SDKs in
[golemcloud/golem](https://github.com/golemcloud/golem) (`sdks/rust` and `sdks/ts`). These SDKs
share a layered architecture:

1. **WIT bindings layer** — generated code implementing the `golem:agent/guest` WIT world exports
   (`initialize`, `invoke`, `discover-agent-types`, `get-definition`) and
   `golem:api/save-snapshot` / `golem:api/load-snapshot`.
2. **Registry layer** — a global agent registry where agent types are registered at startup;
   the WIT exports delegate to the registry to find and invoke agents.
3. **Serialization layer** — `IntoValue` / `FromValueAndType` traits (Rust) that convert between
   user types and the `WitValue` tree structure. In TypeScript, a build-time `typegen` tool
   statically analyzes source to produce a JSON metadata file.
4. **User-facing layer** — proc-macro attributes (`#[agent_definition]`, `#[agent_implementation]`)
   in Rust, or `@agent()` decorator + `BaseAgent` class in TypeScript. Users never see WIT.
5. **Durability layer** — `Durability<SOk, SErr>` wrapper that checks if execution is in live or
   replay mode, persisting or replaying side-effect results via the oplog.

## Repository Structure

```
moonbit-golem/
├── AGENTS.md                          # This file
├── TODO.md                            # Remaining work items
├── PROBLEMS.md                        # MoonBit ecosystem issues encountered
├── golem_sdk/                         # The SDK library (will be published)
│   ├── moon.mod.json                  # Module: vigoo/golem_sdk
│   ├── moon.pkg                       # Root package (currently empty)
│   ├── wit/                           # WIT definitions
│   │   ├── main.wit                   # The `agent-guest` world definition
│   │   ├── deps.toml                  # WIT dependency source (golemcloud/golem main)
│   │   └── deps/                      # Downloaded WIT dependencies
│   ├── interface/                     # WIT-generated MoonBit types and host-import bindings
│   │   ├── golem/
│   │   │   ├── agent/common/          # Core types: AgentType, DataValue, DataSchema, AgentError, etc.
│   │   │   ├── agent/host/            # Host import: get_all_agent_types
│   │   │   ├── api/host/              # Host import: oplog, promises, persistence level
│   │   │   ├── api/context/           # Execution context
│   │   │   ├── api/oplog/             # Oplog types (WrappedFunctionType, OplogIndex, etc.)
│   │   │   ├── durability/durability/ # Durability: begin/end durable function, persist, replay
│   │   │   ├── rpc/types/             # WitValue, WitType, WitNode, WitTypeNode, WasmRpc, etc.
│   │   │   └── rdbms/                 # RDBMS (Postgres, MySQL) bindings
│   │   └── wasi/
│   │       ├── blobstore/             # Blob storage
│   │       ├── clocks/                # Wall clock, monotonic clock
│   │       ├── config/                # Configuration store
│   │       ├── io/                    # Streams, poll, error
│   │       ├── keyvalue/              # Key-value store
│   │       └── logging/               # Logging
│   ├── gen/                           # WIT-generated WASM export glue code
│   │   ├── ffi.mbt                    # mbt_ffi_cabi_realloc, return_area, malloc/free/ptr helpers
│   │   ├── gen_interface_golem_agent_guest_export.mbt  # WASM export stubs for agent guest interface
│   │   ├── gen_interface_golem_api_load_snapshot_export.mbt
│   │   ├── gen_interface_golem_api_save_snapshot_export.mbt
│   │   ├── interface/golem/agent/guest/
│   │   │   └── stub.mbt              # SDK implementation of initialize/invoke/get_definition/discover
│   │   ├── interface/golem/api/saveSnapshot/
│   │   │   └── stub.mbt              # Snapshot save (currently returns [])
│   │   └── interface/golem/api/loadSnapshot/
│   │       └── stub.mbt              # Snapshot load (currently returns Err)
│   ├── world/                         # WIT-generated world-level bindings
│   │   └── agentGuest/                # agent-guest world imports and type re-exports
│   ├── agents/                        # SDK's agent registry
│   │   ├── agents.mbt                 # AgentState, RegisteredAgent, RawAgent trait, register_agent
│   │   └── types/                     # User-facing unstructured data types
│   │       ├── top.mbt                # UnstructuredText, UnstructuredBinary enums + constructors
│   │       ├── schema.mbt             # HasElementSchema/FromElementValue/ToElementValue impls
│   │       └── tests.mbt              # Roundtrip and schema tests
│   ├── builder/                       # WitValue & WitType builder API
│   │   ├── top.mbt                    # Builder struct, primitive add_* methods, build()
│   │   ├── item_builder.mbt           # ItemBuilder for single-child nodes (option, result, variant)
│   │   ├── child_items_builder.mbt    # ChildItemsBuilder for multi-child nodes (record, tuple, list)
│   │   ├── type_builder.mbt           # TypeBuilder for constructing WitType trees
│   │   └── tests.mbt                  # Builder tests
│   ├── extractor/                     # WitValue extractor (deserialization from WitNode tree)
│   │   ├── top.mbt                    # Extractor trait, WitValueExtractor, NodeExtractor impls
│   │   └── tests.mbt                  # Extractor tests
│   └── schema/                        # Schema traits & primitive/compound impls
│       ├── schema.mbt                 # HasElementSchema, FromExtractor, FromElementValue, ToElementValue traits + SchemaOptions
│       ├── primitives.mbt             # Impls for String, Bool, Int, UInt, Int64, UInt64, Float, Double, Byte, Char
│       ├── compounds.mbt             # Impls for Option[T], Array[T], Result[T, E]
│       ├── records.mbt               # make_record_schema/value, extract_field, enum/variant helpers
│       ├── schema_test.mbt           # Schema tests (primitives, compounds, roundtrips)
│       └── records_test.mbt          # Record/enum/variant schema and roundtrip tests
├── golem_sdk_tools/                   # Code generation tools (native CLI, not WASM)
│   ├── moon.mod.json                  # Module: vigoo/golem_sdk_tools (deps: moonbitlang/x, moonbitlang/parser, moonbitlang/formatter)
│   ├── lib/                           # Library package
│   │   ├── mbti.mbt                   # Parser for .mbt source files (extracts pub fn signatures)
│   │   ├── reexports.mbt             # AST construction: generates reexport wrapper functions
│   │   ├── agents.mbt                # Agent source parser: finds #derive.agent structs, constructors, methods
│   │   ├── agents_emit.mbt           # Agent code emitter: generates registration, RawAgent impls as AST
│   │   ├── value_types.mbt           # Value type parser: finds #derive.golem_schema types (records, enums, variants)
│   │   ├── value_types_emit.mbt      # Value type code emitter: generates HasElementSchema/FromExtractor/ToElementValue impls
│   │   ├── ast_helpers.mbt           # AST construction helpers (make_type, make_expr, make_pattern, etc.)
│   │   ├── pkg.mbt                   # moon.pkg parser/updater: parses exports, updates link section
│   │   ├── mbti_test.mbt             # Tests for source parsing
│   │   ├── reexports_test.mbt        # Tests for reexport generation
│   │   ├── agents_test.mbt           # Tests for agent parsing and emission
│   │   ├── value_types_test.mbt      # Tests for value type parsing
│   │   ├── value_types_emit_test.mbt # Tests for value type emission
│   │   └── pkg_test.mbt             # Tests for moon.pkg parsing/updating
│   └── cmd/                           # CLI entry point
│       └── main.mbt                   # `reexports` and `agents` subcommands
└── golem_sdk_example1/                # Example consumer project
    ├── moon.mod.json                  # Module: vigoo/golem_sdk_example1 (deps on local golem_sdk)
    ├── build.sh                       # Build script: reexports + agents codegen + moon build + wasm-tools
    ├── golem.yaml                     # Golem 1.4.2 application definition with build pipeline
    └── counter/                       # Example agents: Counter and TaskManager
        ├── moon.pkg                   # is-main, WASM export link config (auto-updated by tools)
        ├── counter.mbt               # Counter agent: #derive.agent struct with increment/decrement/get_value
        ├── task_manager.mbt          # TaskManager agent: custom types with #derive.golem_schema
        ├── golem_reexports.mbt       # Generated — re-exports WASM entry points from SDK gen package
        ├── golem_derive.mbt          # Generated — HasElementSchema/FromExtractor/ToElementValue for custom types
        └── golem_agents.mbt          # Generated — agent registration, RawAgent impls, init block
```

## Key Concepts

### WIT Bindgen

All code under `interface/`, `world/`, and `gen/` (except `gen/interface/*/stub.mbt`) is
**auto-generated** by `wit-bindgen moonbit`. Do NOT edit these files. Regenerate with:

```sh
cd golem_sdk
wit-bindgen moonbit ./wit --derive-show --derive-eq --derive-error --project-name vigoo/golem_sdk --ignore-stub
moon fmt
```

Note: `moon run script bindgen` is defined in `moon.mod.json` but can fail if the project is in a
broken state (moon tries to resolve packages before running the script). In that case, run
`wit-bindgen` directly as shown above.

After regeneration, `wit-bindgen` produces `moon.pkg.json` files; `moon fmt` converts them to the
new `moon.pkg` plain-text format. The `--ignore-stub` flag means `wit-bindgen` will NOT regenerate
the stub files or their `moon.pkg` files — those must be maintained by hand.

The sub-packages under `gen/` (`gen/interface/golem/agent/guest/`, `gen/interface/golem/api/loadSnapshot/`,
`gen/interface/golem/api/saveSnapshot/`, `gen/world/agentGuest/`) need their own `moon.pkg` files
with correct imports. Since `--ignore-stub` skips these, they must be created/maintained manually.

The `stub.mbt` files under `gen/interface/` are the **SDK's implementation** of the WIT export
interfaces. These are the files where we write the SDK's dispatch logic. `wit-bindgen` generates them
once (with `--ignore-stub` preventing overwrites), and we maintain them by hand.

FFI helper functions (`mbt_ffi_malloc`, `mbt_ffi_free`, `mbt_ffi_ptr2str`, etc.) are inlined by
`wit-bindgen` into each package's `ffi.mbt` that needs them. The `gen/ffi.mbt` file contains
`mbt_ffi_cabi_realloc` (the Component Model's canonical ABI allocator) and the shared `return_area`.

### The Agent Registry Pattern

The core pattern (in `agents/agents.mbt`):

1. A global `AgentState` holds a `HashMap[String, RegisteredAgent]` and a single `AgentInstance?`.
2. Each agent type calls `register_agent(...)` during `init {}` to register itself.
3. When the host calls `initialize(agent_type, input)`, the SDK looks up the registered agent by
   name, calls its `construct` function, and stores the resulting `AgentInstance`.
4. When the host calls `invoke(method_name, input)`, the SDK delegates to the instance's
   `RawAgent::invoke` method.
5. `discover_agent_types()` returns all registered agent types.
6. `get_definition()` returns the current instance's type.

### The RawAgent Trait

```moonbit
pub(open) trait RawAgent {
  invoke(Self, method_name : String, input : DataValue) -> Result[DataValue, AgentError]
}
```

This is the low-level interface every agent must implement. The `agents` code generation tool
auto-generates `RawAgent` impls with method dispatch, constructor deserialization, and result
serialization.

### Data Types (WIT ↔ MoonBit Mapping)

The serialization bridge between user types and WIT uses these key types from
`interface/golem/rpc/types/`:

| WIT Concept | MoonBit Type | Purpose |
|---|---|---|
| Value tree | `WitValue { nodes: Array[WitNode] }` | Runtime value representation |
| Type tree | `WitType { nodes: Array[NamedWitTypeNode] }` | Type description for schema |
| Node value | `WitNode` enum (22 variants) | One node in a value tree |
| Node type | `WitTypeNode` enum (22 variants) | One node in a type tree |
| Value+Type | `ValueAndType { value, typ }` | Self-describing value |

The agent-level types from `interface/golem/agent/common/`:

| Type | Purpose |
|---|---|
| `DataValue` | Tuple or Multimodal collection of `ElementValue`s |
| `DataSchema` | Tuple or Multimodal collection of `(name, ElementSchema)` pairs |
| `ElementValue` | `ComponentModel(WitValue)` or unstructured text/binary |
| `ElementSchema` | `ComponentModel(WitType)` or unstructured text/binary descriptors |
| `AgentType` | Full agent definition: name, description, constructor, methods, dependencies |
| `AgentMethod` | Method schema: name, description, input/output schemas |
| `AgentConstructor` | Constructor schema: name, description, input schema |
| `AgentError` | Error type with InvalidInput/InvalidMethod/InvalidType/CustomError variants |

### Builder (WitValue Construction)

The `builder/` package provides a fluent API for constructing `WitValue` and `WitType` trees:

- **`Builder`** — builds `WitValue` trees. Has convenience methods for all primitive types
  (`u8`, `s32`, `string`, `bool`, etc.) and compound types (`record`, `list`, `option_some`,
  `option_none`, `result_ok`, `result_err`, `variant`, `tuple`, `flags`, `enum_value`, `handle`).
  Uses a callback-based nesting pattern with `ItemBuilder` and `ChildItemsBuilder`.
- **`TypeBuilder`** — builds `WitType` trees. Methods: `option_type`, `list_type`, `result_type`,
  `record_type`, `variant_type`, `enum_type`. Both builders handle node index rebasing when
  composing sub-trees via `add_wit_value` / `add_wit_type`.
- **`ItemBuilder`** — used inside callbacks for single-child container nodes (option, result, variant).
  Mirrors all `Builder` methods but delegates to the parent builder.
- **`ChildItemsBuilder`** — used inside callbacks for multi-child container nodes (record, tuple, list).
  Collects child node indices and finalizes them on the parent.
- **`BuilderError`** — suberror for builder misuse (e.g., adding to a closed builder).

### Extractor (WitValue Deserialization)

The `extractor/` package provides a trait-based API for reading values from `WitValue` trees:

- **`Extractor` trait** — 21-method open trait with accessors for all WIT node types:
  `u8()`, `s32()`, `string()`, `field(idx)`, `variant()`, `enum_value()`, `flags()`,
  `tuple_element(idx)`, `list_elements()`, `option()`, `result()`, `handle()`, etc.
  Returns `Option` types (`None` on type mismatch).
- **`WitValueExtractor`** — implements `Extractor` for a `WitValue` (delegates to root node).
- **`NodeExtractor`** — implements `Extractor` for a single `WitNode` within a `WitValue` context.
- **`extract(WitValue) -> &Extractor`** — entry point for extraction.
- **`extract_component_model_value(ElementValue) -> &Extractor`** — unwraps
  `ElementValue::ComponentModel` and extracts.
- **`extract_tuple`, `extract_multimodal`, `expect_single_element`** — helpers for `DataValue`.

### Schema Traits (Serialization Layer)

The `schema/` package defines the serialization traits and provides implementations for all
primitive and compound MoonBit types. This is the SDK's equivalent of the Rust SDK's
`IntoValue`/`FromValueAndType`.

**Traits** (in `schema.mbt`):

| Trait | Purpose |
|---|---|
| `HasElementSchema` | Returns the `ElementSchema` (WitType) for a type. Static method. |
| `FromExtractor` | Deserializes from an `&Extractor` (low-level, works at WitNode level). |
| `FromElementValue` | Deserializes from an `ElementValue` (convenience, wraps `FromExtractor`). |
| `ToElementValue` | Serializes to an `ElementValue`. |

**Helper functions**:
- `schema_of(v)` — infers `HasElementSchema` from a value
- `schema_of_tag(TypeTag[T], options~)` — gets schema for a type without needing a value instance;
  accepts optional `SchemaOptions` for applying language/MIME restrictions to unstructured types
- `from_element_value_as[T](ElementValue) -> T` — typed deserialization
- `to_element_value_as[T](v) -> ElementValue` — typed serialization
- `from_extractor_as[T](&Extractor) -> T` — typed low-level deserialization

**SchemaOptions** (`schema.mbt`):
- `SchemaOptions { text_languages, binary_mime_types }` — passed to `schema_of_tag` to apply
  restrictions. When `text_languages` is non-empty and the base schema is `UnstructuredText`,
  the restrictions are injected into the `TextDescriptor`. Similarly for `binary_mime_types` /
  `UnstructuredBinary`. Options are silently ignored for `ComponentModel` schemas.

**Primitive implementations** (`primitives.mbt`):
All four traits are implemented for: `String`, `Bool`, `Int` (S32), `UInt` (U32), `Int64` (S64),
`UInt64` (U64), `Float` (F32), `Double` (F64), `Byte` (U8), `Char`.

**Compound implementations** (`compounds.mbt`):
All four traits are implemented for: `Option[T]`, `Array[T]`, `Result[T, E]` (with appropriate
trait bounds on type parameters).

**Record/Enum/Variant helpers** (`records.mbt`):
Used by generated code for custom user types:
- `make_record_schema(fields)` / `make_record_value(fields)` / `extract_field[T](e, idx)`
- `make_enum_schema(cases)` / `make_enum_value(idx)` / `extract_enum(e)`
- `make_variant_schema(cases)` / `make_variant_value(case_idx, payload)` / `extract_variant(e)`

### Code Generation (golem_sdk_tools)

The `golem_sdk_tools` CLI provides two subcommands that automate boilerplate generation:

#### `reexports` subcommand

Generates `golem_reexports.mbt` — re-exports all WASM entry points (`cabi_realloc`,
`wasmExport*` functions) from the SDK's `gen` package. Also auto-updates the target `moon.pkg`
file's `link.wasm.exports` section.

```sh
cd golem_sdk_tools
moon run cmd -- reexports <sdk-path> <target-dir>
# e.g.: moon run cmd -- reexports ../golem_sdk ../golem_sdk_example1/counter
```

The tool parses `.mbt` source files in the SDK's `gen/` directory to discover exported functions
(public `fn` declarations matching `wasmExport*` or `mbt_ffi_cabi_realloc`), constructs AST nodes
via `moonbitlang/parser`, and emits MoonBit source via `moonbitlang/formatter`.

It also parses the SDK's `gen/moon.pkg` to extract the `link.wasm.exports` entries, transforms
them (stripping `mbt_ffi_` prefixes), and updates the target `moon.pkg` with the correct link
section (creating or replacing it as needed).

#### `agents` subcommand

Generates two files from user source code annotations:

1. **`golem_agents.mbt`** — agent registration and dispatch code:
   - `fn init {}` block that calls `register_agent(...)` for each agent
   - `AgentType` definitions with schemas derived from method signatures
   - Constructor deserialization (extracts tuple elements, deserializes via `@schema`)
   - `impl RawAgent for AgentName` with method dispatch (`match method_name { ... }`)
   - Parameter deserialization and result serialization using `@schema` traits

2. **`golem_derive.mbt`** — serialization impls for custom data types:
   - `impl HasElementSchema` — generates schema using `make_record_schema` / `make_enum_schema` / `make_variant_schema`
   - `impl FromExtractor` — generates field-by-field extraction for records, case matching for enums/variants
   - `impl FromElementValue` — boilerplate delegation to `FromExtractor`
   - `impl ToElementValue` — generates field-by-field serialization for records, case matching for enums/variants

```sh
cd golem_sdk_tools
moon run cmd -- agents <package-dir>
# e.g.: moon run cmd -- agents ../golem_sdk_example1/counter
```

**Source annotations recognized:**

- `#derive.agent` on a struct — marks it as a Golem agent. Supports `#derive.agent("ephemeral")`
  for ephemeral mode (default is durable).
- `#derive.golem_schema` on a struct or enum — generates serialization impls for custom data types.
- `#derive.prompt_hint("...")` on methods — adds a prompt hint to the method's agent definition.
- `#derive.text_languages("param_name", "en", "de")` on methods — applies language restrictions
  to an `UnstructuredText` parameter's schema.
- `#derive.mime_types("param_name", "image/png", "image/jpeg")` on methods — applies MIME type
  restrictions to an `UnstructuredBinary` parameter's schema.
- Doc comments (`///`) on structs, constructors, and methods are extracted as descriptions.

**Agent parsing** (`agents.mbt`):
- Finds structs annotated with `#derive.agent`
- Finds the `::new` constructor (required) and extracts parameters
- Finds all public methods with `Self` as first parameter
- Extracts return types, parameter types, doc strings, mode, prompt hints, and schema restrictions
- Supports types: `Simple(name)`, `Optional(T)`, `List(T)`, `ResultType(T, E)`, `Tuple(elems)`
- Validates that `UnstructuredText`/`UnstructuredBinary` are not nested inside `Option`/`Array`/
  `Result`/`Tuple` — they must be direct parameter or return types
- Validates that `#derive.text_languages` / `#derive.mime_types` annotations reference existing
  parameters of the correct unstructured type

**Value type parsing** (`value_types.mbt`):
- Finds types annotated with `#derive.golem_schema`
- Supports three kinds: `Record` (struct fields), `SimpleEnum` (all-unit enum), `VariantEnum` (enum with payloads)
- Variant payloads can be: `None` (unit), `Single(type)`, or `Multi(fields)` (record-like)

**Code emission** (`agents_emit.mbt`, `value_types_emit.mbt`):
Both emitters construct `@syntax.Impl` AST nodes using helpers from `ast_helpers.mbt`, then
serialize them via `@formatter.impls_to_string`. The `ast_helpers.mbt` file provides ~50 helper
functions for constructing AST nodes (types, expressions, patterns, match cases, etc.).

**Architecture note**: The `golem_sdk_tools` now uses `moonbitlang/formatter` (via a local path
dependency pointing to `../../moonbit-formatter`) for code emission, and `moonbitlang/parser` for
both parsing and AST construction. The earlier custom emitter was replaced once the formatter
dependency became available via a local path workaround.

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

The `fn main {}` block must exist in the main package (can be empty). Multiple agents can
coexist in the same package — each gets registered in the generated `fn init {}` block.

### Durability

The durability system (in `interface/golem/durability/durability/`) provides:

- `current_durable_execution_state()` → `{ is_live, persistence_level }`
- `begin_durable_function(function_type)` → `oplog_index`
- `end_durable_function(function_type, begin_index, forced_commit)`
- `persist_durable_function_invocation(name, request, response, function_type)`
- `persist_typed_durable_function_invocation(name, request, response, function_type)`
- `read_persisted_durable_function_invocation(begin_index)` / typed variant

The pattern for wrapping any side-effecting call:
1. Call `begin_durable_function` to get the oplog position
2. Check `is_live` from `current_durable_execution_state()`
3. If live: execute the real operation, then `persist_*` the result
4. If replaying: call `read_persisted_*` to get the cached result
5. Call `end_durable_function`

## Current State & What Works

- WIT bindings are fully generated and compile for the `wasm` target (updated to Golem 1.4.2 WITs)
- The agent registry pattern is implemented
- **Builder/Extractor packages are complete** — fluent API for constructing and reading `WitValue`/`WitType` trees, with comprehensive tests
- **Schema traits are implemented** — `HasElementSchema`, `FromExtractor`, `FromElementValue`, `ToElementValue` for all MoonBit primitives and compound types (`Option[T]`, `Array[T]`, `Result[T, E]`), plus record/enum/variant helpers for custom types
- **Agent code generation is complete** — the `agents` subcommand parses `#derive.agent` and `#derive.golem_schema` annotations, generates `golem_agents.mbt` (registration + RawAgent dispatch) and `golem_derive.mbt` (serialization impls)
- **Reexport generation is complete** — the `reexports` subcommand generates `golem_reexports.mbt` and auto-updates `moon.pkg` link sections
- **Two working example agents** — `Counter` (simple state, primitive types) and `TaskManager` (custom types: `Priority` enum, `TaskInfo` struct with optional fields)
- The build pipeline works: codegen → `moon build --target wasm` → `wasm-tools component embed` → `wasm-tools component new`
- A `golem.yaml` application definition is set up for Golem 1.4.2 with the full build pipeline
- Snapshot save/load stubs exist but are not yet functional
- Agent mode (durable/ephemeral) is supported via `#derive.agent("ephemeral")`
- Prompt hints on methods via `#derive.prompt_hint("...")`
- **Unstructured text/binary types are supported** — `UnstructuredText` and `UnstructuredBinary`
  enums in `agents/types/` with full schema trait impls. Uses `Bytes` for binary data (idiomatic
  MoonBit). The code generator treats them like any other type via trait dispatch (no name-based
  recognition). Language/MIME restrictions via `#derive.text_languages`/`#derive.mime_types`
  annotations on methods, validated at codegen time. Nesting inside `Option`/`Array`/`Result`/
  `Tuple` is detected and rejected with clear error messages.

## What Needs To Be Built

### 1. Custom Data Types — Extended Support

The current `#derive.golem_schema` supports structs (records), simple enums (all-unit), and
variant enums (with payloads). Still needed:
- Variant enums with multi-field (record-like) payloads
- Tuple types
- Nested custom types across packages (currently must be in the same package)

### 2. Snapshot Support

Implement `save` and `load` for agent state persistence. Requires a serialization format (JSON or
binary) and the ability to serialize/deserialize the agent struct. Could leverage the same
schema traits or MoonBit's `ToJson`/`FromJson`.

### 3. Durability Wrapper

A high-level `Durability` struct/module that wraps the low-level durability FFI calls into an
ergonomic API, similar to Rust's `Durability<SOk, SErr>`.

### 4. Host API Re-exports

Provide ergonomic re-exports of commonly used host APIs (logging, key-value store, blob storage,
config, LLM, etc.) so users import from `@golem_sdk` instead of deep WIT-generated paths.

### 5. RPC Support

Inter-component communication via the `golem:rpc` types (`WasmRpc`, etc.).

### 6. Template Generalization

Generalize the example directory as a reusable MoonBit Golem template with proper variable
substitution.

### 7. Golem 1.5 Update

Update WIT definitions and SDK to Golem 1.5 when available.

### 8. Code-First Endpoints & Config

Support for defining REST endpoints and configuration schemas from code.

## Build & Test Commands

```sh
# In golem_sdk/:
moon check --target wasm          # Type-check SDK
moon build --target wasm          # Build SDK
moon test                         # Run tests (builder, extractor, schema tests)
moon fmt                          # Format code
moon info                         # Regenerate .mbti files

# Regenerate WIT bindings:
moon run script bindgen

# In golem_sdk_tools/:
moon check                        # Type-check tools (native target)
moon build                        # Build tools
moon test                         # Run tests
moon run cmd -- reexports <sdk-path> <target-dir>  # Generate reexports + update moon.pkg
moon run cmd -- agents <package-dir>               # Generate golem_agents.mbt + golem_derive.mbt

# In golem_sdk_example1/:
moon check --target wasm          # Type-check example
./build.sh                        # Full build: codegen + moon build + wasm-tools

# The resulting component WASM is at:
# golem_sdk_example1/_build/wasm/release/counter.agent.wasm
```

## Coding Conventions

- MoonBit blocks separated by `///|` — order is irrelevant
- Follow existing naming: `snake_case` for functions/values, `UpperCamelCase` for types/enums
- Files generated by `wit-bindgen` are marked `// Generated by wit-bindgen ... DO NOT EDIT!`
- Files generated by `golem_sdk_tools` are marked `// Generated by golem_sdk_tools — DO NOT EDIT!`
- SDK stub files (`gen/interface/*/stub.mbt`) ARE maintained by hand despite being in the `gen/` tree
- Use `moon check --target wasm` frequently — the project targets WASM only
- Tests should use `inspect()` with snapshot testing (`moon test --update`)
- Run `moon info && moon fmt` before finalizing changes

## Important Technical Notes

- The SDK targets **WASM only** (`preferred-target: wasm` in `moon.mod.json`)
- String encoding is **UTF-16** (MoonBit's native format, passed to `wasm-tools component embed --encoding utf16`)
- Memory management uses `mbt_ffi_malloc`/`mbt_ffi_free` (inlined per-package) for WASM linear memory, with MoonBit's GC for MoonBit objects
- The `agents` package holds mutable global state (`let state : AgentState = AgentState::new()`) — this is a module-level singleton
- WASM exports are linked via `moon.pkg` link configuration — every agent component must declare these exports (auto-generated by the `reexports` tool)
- The `mbt_ffi_cabi_realloc` function in `gen/ffi.mbt` is the Component Model's canonical ABI allocator
- `moon.pkg` can use either the new format (plain text) or `moon.pkg.json` (JSON) — `moon fmt` converts JSON to plain text
- Generated code files (`golem_reexports.mbt`, `golem_agents.mbt`, `golem_derive.mbt`) should not be manually edited
- Multiple agents can coexist in the same package — each is registered in the single generated `fn init {}` block

## Dependencies & Tools

- **wit-bindgen** ≥ 0.53.1 with `moonbit` backend — generates all WIT bindings
- **wasm-tools** — for `component embed` (adds WIT type info to WASM) and `component new` (creates Component Model WASM)
- **moon** — MoonBit build tool
- **moonbitlang/parser** (0.1.16) — used by `golem_sdk_tools` for parsing source files and AST construction
- **moonbitlang/formatter** (local path `../../moonbit-formatter`) — used by `golem_sdk_tools` for emitting generated MoonBit source from AST
- **moonbitlang/x** (0.4.39) — used by `golem_sdk_tools` for filesystem and env args
