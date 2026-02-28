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
│   └── agents/                        # SDK's agent registry
│       └── agents.mbt                 # AgentState, RegisteredAgent, RawAgent trait, register_agent
├── golem_sdk_tools/                   # Code generation tools (native CLI, not WASM)
│   ├── moon.mod.json                  # Module: vigoo/golem_sdk_tools (deps: moonbitlang/x, moonbitlang/parser)
│   ├── lib/                           # Library package
│   │   ├── mbti.mbt                   # Parser for .mbti files (extracts FnSignature from pub fn lines)
│   │   ├── reexports.mbt             # AST construction: generates @syntax.Impl nodes for re-export wrappers
│   │   ├── emitter.mbt               # Custom AST-to-string serializer (bypasses moonbitlang/formatter dep issues)
│   │   ├── mbti_test.mbt             # Tests for MBTI parsing
│   │   └── reexports_test.mbt        # Tests for reexport generation + emission
│   └── cmd/                           # CLI entry point
│       └── main.mbt                   # `reexports` subcommand: reads SDK mbti, writes golem_reexports.mbt
└── golem_sdk_example1/                # Example consumer project
    ├── moon.mod.json                  # Module: vigoo/golem_sdk_example1 (deps on local golem_sdk)
    ├── build.sh                       # moon build + wasm-tools embed + component new
    └── counter/                       # Example agent: Counter
        ├── moon.pkg                   # is-main, WASM export link config
        ├── counter.mbt               # Hand-written agent: struct, RawAgent impl, registration
        └── golem_reexports.mbt       # Generated by golem_sdk_tools — re-exports WASM entry points
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

This is the low-level interface every agent must implement. Currently users implement it by hand
with a `match` on `method_name`. The goal is to auto-generate this via code generation.

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

- WIT bindings are fully generated and compile for the `wasm` target
- The agent registry pattern is implemented
- A minimal hand-written `Counter` agent works end-to-end (registers, initializes, invokes)
- The build pipeline works: `moon build --target wasm` → `wasm-tools component embed` → `wasm-tools component new`
- Snapshot save/load stubs exist but are not yet functional
- **`golem_sdk_tools reexports` command is complete and tested** — automatically generates
  `golem_reexports.mbt` files that re-export all WASM entry points (`cabi_realloc`,
  `wasmExport*` functions) from the SDK's `gen` package. This eliminates hand-written
  boilerplate in consumer projects. Usage:
  ```sh
  cd golem_sdk_tools
  moon run cmd -- reexports <sdk-path> <target-dir>
  # e.g.: moon run cmd -- reexports ../golem_sdk ../golem_sdk_example1/counter
  ```
  The tool parses `gen/pkg.generated.mbti` to discover exported functions, constructs AST
  nodes via `moonbitlang/parser`, and emits MoonBit source using a custom lightweight emitter
  (bypasses `moonbitlang/formatter` dependency conflicts). Has 9 tests covering MBTI parsing,
  reexport generation, and emission.

  **Architecture note**: The `.mbti` parsing uses string-based processing rather than
  `@mbti_parser` because the parser's grammar has an upstream bug: the `func_sig` rule lacks
  a `vis` prefix, so it cannot parse `pub fn` signatures that `moon info` generates. The AST
  construction uses `moonbitlang/parser/syntax` types. The emitter is a custom `StringBuilder`-
  based serializer because `moonbitlang/formatter` cannot be added as a dependency due to a
  transitive `Yoorkin/ArgParser` version conflict (formatter 0.1.2 pins parser 0.1.11 +
  ArgParser 0.1.11, while we use parser 0.1.16). Once both upstream issues are resolved, the
  tool should switch to: `@mbti_parser` for parsing → AST transform → `@formatter.impls_to_string`
  for emission.

## What Needs To Be Built

### 1. IntoValue / FromValue Serialization Traits

Equivalent to the Rust SDK's `IntoValue` / `FromValueAndType`. These traits convert between
user-defined MoonBit types and `WitValue` / `WitType`.

```moonbit
// Target API (conceptual)
pub(open) trait IntoValue {
  into_value(Self) -> WitValue
  get_type() -> WitType  // static — returns the WitType schema for this type
}

pub(open) trait FromValue {
  from_value(WitValue, WitType) -> Result[Self, String]
}
```

Must provide implementations for all MoonBit primitives: `Bool`, `Int`, `UInt`, `Int64`, `UInt64`,
`Float`, `Double`, `String`, `Byte`, `Char`, `Option[T]`, `Result[T, E]`, `Array[T]`, tuples, etc.

### 2. Code Generation (Custom Derive)

MoonBit doesn't have proc-macros. The approach is a CLI tool using `moonbitlang/parser` and
`moonbitlang/formatter` (see [Yoorkin/custom_derive](https://github.com/Yoorkin/custom_derive)):

- Parse source files, find types annotated with `#derive.into_value` / `#derive.from_value`
- Generate `impl IntoValue for MyType` and `impl FromValue for MyType`
- Generate agent registration code: `RawAgent` impl, `AgentType` construction, `register_agent` call
- Mark generated code with `#derive.generated` for idempotent re-runs

The derive tool would be a separate MoonBit package (`derive/` or `cmd/`) that runs as a pre-build step.

### 3. Agent Definition Abstraction

Replace the current hand-written boilerplate in user code. The Rust SDK uses
`#[agent_definition]` on a trait + `#[agent_implementation]` on an impl block. For MoonBit,
the equivalent could be:

```moonbit
// User writes this:
#derive.agent
pub(all) struct Counter {
  name : String
  mut value : UInt64
}

// With annotated methods:
#derive.agent_method("increment", description="Increments the counter")
pub fn Counter::increment(self : Self) -> Unit { ... }

#derive.agent_constructor(description="Creates a new counter")
pub fn Counter::new(name : String) -> Counter { ... }
```

The derive tool would generate:
- `impl RawAgent for Counter` with method dispatch
- `AgentType` definition with schemas derived from method signatures
- `register_agent(...)` call in an `init {}` block
- WIT export re-export functions

### 4. WIT Export Boilerplate Elimination

~~Currently the example (`counter.mbt`) must manually re-export all `@gen.wasmExport*` functions~~
**Partially solved**: The `golem_sdk_tools reexports` command now auto-generates `golem_reexports.mbt`
with all WASM entry point re-exports. The `moon.pkg` link configuration still needs to be duplicated
in each consumer project. A future improvement could auto-generate the `moon.pkg` link section too.

### 5. Snapshot Support

Implement `save` and `load` for agent state persistence. Requires a serialization format (JSON or
binary) and the ability to serialize/deserialize the agent struct. Could leverage the same
`IntoValue`/`FromValue` traits or MoonBit's `ToJson`/`FromJson`.

### 6. Durability Wrapper

A high-level `Durability` struct/module that wraps the low-level durability FFI calls into an
ergonomic API, similar to Rust's `Durability<SOk, SErr>`.

### 7. Host API Re-exports

Provide ergonomic re-exports of commonly used host APIs (logging, key-value store, blob storage,
config, LLM, etc.) so users import from `@golem_sdk` instead of deep WIT-generated paths.

## Build & Test Commands

```sh
# In golem_sdk/:
moon check --target wasm          # Type-check SDK
moon build --target wasm          # Build SDK
moon test                         # Run tests (non-wasm tests)
moon fmt                          # Format code
moon info                         # Regenerate .mbti files

# Regenerate WIT bindings:
moon run script bindgen

# In golem_sdk_tools/:
moon check                        # Type-check tools (native target)
moon build                        # Build tools
moon test                         # Run tests (9 tests: MBTI parsing, reexport generation, emission)
moon run cmd -- reexports <sdk-path> <target-dir>  # Generate reexports

# In golem_sdk_example1/:
moon check --target wasm          # Type-check example
./build.sh                        # Full build: moon build + wasm-tools embed + component new

# The resulting component WASM is at:
# golem_sdk_example1/target/wasm/release/counter.agent.wasm
```

## Coding Conventions

- MoonBit blocks separated by `///|` — order is irrelevant
- Follow existing naming: `snake_case` for functions/values, `UpperCamelCase` for types/enums
- Files generated by `wit-bindgen` are marked `// Generated by wit-bindgen ... DO NOT EDIT!`
- SDK stub files (`gen/interface/*/stub.mbt`) ARE maintained by hand despite being in the `gen/` tree
- Use `moon check --target wasm` frequently — the project targets WASM only
- Tests should use `inspect()` with snapshot testing (`moon test --update`)
- Run `moon info && moon fmt` before finalizing changes

## Important Technical Notes

- The SDK targets **WASM only** (`preferred-target: wasm` in `moon.mod.json`)
- String encoding is **UTF-16** (MoonBit's native format, passed to `wasm-tools component embed --encoding utf16`)
- Memory management uses `mbt_ffi_malloc`/`mbt_ffi_free` (inlined per-package) for WASM linear memory, with MoonBit's GC for MoonBit objects
- The `agents` package holds mutable global state (`let state : AgentState = AgentState::new()`) — this is a module-level singleton
- WASM exports are linked via `moon.pkg` link configuration — every agent component must declare these exports
- The `mbt_ffi_cabi_realloc` function in `gen/ffi.mbt` is the Component Model's canonical ABI allocator
- `moon.pkg` can use either the new format (plain text) or `moon.pkg.json` (JSON) — `moon fmt` converts JSON to plain text

## Dependencies & Tools

- **wit-bindgen** ≥ 0.53.1 with `moonbit` backend — generates all WIT bindings
- **wasm-tools** — for `component embed` (adds WIT type info to WASM) and `component new` (creates Component Model WASM)
- **moon** — MoonBit build tool
- **moonbitlang/parser** (0.1.16) — used by `golem_sdk_tools` for AST construction; `moonbitlang/formatter` is NOT used (bypassed with custom emitter due to dependency conflicts)
- **moonbitlang/x** (0.4.39) — used by `golem_sdk_tools` for filesystem and env args
