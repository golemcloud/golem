# Golem SDK Tools for MoonBit

Code generation tools for the [Golem SDK for MoonBit](https://mooncakes.io/docs/#/golemcloud/golem_sdk/). Parses MoonBit source annotations and generates the boilerplate code that connects agent definitions to the Golem runtime.

## Commands

### `reexports`

Generates `golem_reexports.mbt` and updates the target package's `moon.pkg` link section with WASM export declarations.

```sh
moon run cmd -- reexports <sdk-path> <target-dir>
```

**What it does:**
1. Scans the SDK's `gen/` directory for exported functions (`wasmExport*`, `mbt_ffi_cabi_realloc`)
2. Generates wrapper functions that re-export them from the user's package
3. Parses the SDK's `gen/moon.pkg` to extract link export entries and merges them into the target `moon.pkg`

### `agents`

Generates agent registration, serialization, and RPC client code from source annotations.

```sh
moon run cmd -- agents <package-dir>
```

**What it generates:**

| File | Contents |
|---|---|
| `golem_agents.mbt` | `fn init {}` block registering all agents, `AgentTypeDef` definitions, constructor decoding, `impl RawAgent` with method dispatch |
| `golem_derive.mbt` | `@schema.IntoSchema` / `@schema.FromSchema` impls for `#derive.golem_schema` types; `@multimodal.MultimodalModality` impls for `#derive.multimodal` enums |
| `golem_clients.mbt` | RPC client structs (`<AgentName>Client`) with awaited, fire-and-forget (`trigger_*`), and scheduled (`schedule_*`) method variants |

It also auto-adds the required imports to the target `moon.pkg` (e.g. `agents`, `schema`, `schema_model`, `interface/golem/agent/common`, `interface/golem/core/types`, `rpc`, `multimodal`, `config`). The `reexports` subcommand additionally adds the `gen` import.

## Supported Annotations

| Annotation | Target | Purpose |
|---|---|---|
| `#derive.agent` | struct | Marks a struct as a Golem agent |
| `#derive.agent("ephemeral")` | struct | Marks an agent as ephemeral (stateless) |
| `#derive.golem_schema` | struct, enum | Generates serialization impls |
| `#derive.multimodal` | enum | Generates `@multimodal.MultimodalModality` trait impl |
| `#derive.prompt_hint("...")` | method | Adds a prompt hint to the method definition |

Doc comments (`///`) on structs, constructors, and methods are extracted as descriptions in the generated `AgentType` metadata.

## Usage with `golem.yaml`

Typically invoked as build steps in a Golem application manifest:

```yaml
build:
  - command: moon run cmd -- reexports ../golem_sdk ../my_app/my_component
    dir: ../golem_sdk_tools
  - command: moon run cmd -- agents ../my_app/my_component
    dir: ../golem_sdk_tools
  - command: moon build --target wasm --release
  # ... wasm-tools component embed/new steps
```

## Requirements

- MoonBit toolchain (`moon`)
- `moonbitlang/parser` — for parsing source files and constructing AST
- `moonbitlang/formatter` — for emitting generated MoonBit source

## License

Apache-2.0
