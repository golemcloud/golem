# Golem SDK for MoonBit

Build durable, fault-tolerant agents on the [Golem](https://golem.cloud) platform using [MoonBit](https://moonbitlang.com).

## Overview

This SDK lets you write Golem agents in MoonBit with minimal boilerplate. Agents are compiled to WebAssembly components and run on Golem, which provides automatic durable execution, persistent state, and agent-to-agent communication.

## Quick Start

### 1. Define an agent

Annotate a struct with `#derive.agent`, provide a `::new` constructor, and add public methods:

```moonbit
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

/// Returns the current value
pub fn Counter::get_value(self : Self) -> UInt64 {
  self.value
}
```

### 2. Use custom data types

Annotate structs and enums with `#derive.golem_schema` to make them usable as method parameters and return types:

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

### 3. Build and deploy

Use `golem build` and `golem deploy` with a `golem.yaml` application manifest. See the [example project](https://github.com/golemcloud/moonbit-sdk/tree/main/golem_sdk_example1) for a complete setup.

## Features

- **Agent registry** — register multiple agent types in a single component via `#derive.agent`
- **Automatic serialization** — `#derive.golem_schema` generates `HasElementSchema`, `FromExtractor`, `FromElementValue`, and `ToElementValue` impls for custom types (structs, enums, variants)
- **Multimodal input** — accept mixed text, binary, and custom modality data via `#derive.multimodal` and `Multimodal[T]`
- **Unstructured data** — `UnstructuredText` and `UnstructuredBinary` types with optional language/MIME restrictions
- **Agent-to-agent RPC** — auto-generated client stubs (`CounterClient`) with awaited, fire-and-forget, and scheduled invocations
- **Logging** — structured logging via `@logging.with_name("my-agent")` with level filtering
- **Tracing** — span-based tracing via `@context.with_span(...)` with attributes
- **Ephemeral mode** — `#derive.agent("ephemeral")` for stateless per-invocation agents
- **Prompt hints** — `#derive.prompt_hint("...")` to annotate methods with LLM-friendly descriptions

## Packages

| Package | Description |
|---|---|
| `agents` | Agent registry, `RawAgent` trait, `register_agent` |
| `agents/types` | `UnstructuredText`, `UnstructuredBinary`, `Multimodal[T]` types |
| `schema` | Serialization traits and impls for primitives, `Option`, `Array`, `Result` |
| `builder` | Fluent API for constructing `WitValue` and `WitType` trees |
| `extractor` | Trait-based API for reading values from `WitValue` trees |
| `logging` | Structured logging with named loggers and level filtering |
| `context` | Span-based tracing and invocation context |
| `rpc` | Agent-to-agent RPC helpers |
| `interface/` | WIT-generated bindings (Golem host APIs, WASI interfaces) |
| `gen/` | WIT-generated WASM export glue code |

## Code Generation

This SDK is designed to be used with [`golemcloud/golem_sdk_tools`](https://mooncakes.io/docs/#/golemcloud/golem_sdk_tools/), which generates the boilerplate code that connects your agent definitions to the Golem runtime:

- `golem_reexports.mbt` — re-exports WASM entry points from the SDK
- `golem_agents.mbt` — agent registration, constructor deserialization, method dispatch
- `golem_derive.mbt` — serialization impls for `#derive.golem_schema` types
- `golem_clients.mbt` — RPC client stubs for agent-to-agent calls

## Requirements

- MoonBit toolchain (`moon`)
- Golem CLI (`golem`) version 1.4.x+
- `wasm-tools` for component model linking
- `golemcloud/golem_sdk_tools` for code generation

## Documentation

- [Golem Documentation](https://learn.golem.cloud)
- [MoonBit Documentation](https://docs.moonbitlang.com)
- [Example Project](https://github.com/golemcloud/moonbit-sdk/tree/main/golem_sdk_example1)

## License

Apache-2.0
