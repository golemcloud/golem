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

Generates agent and tool registration, serialization, dispatch, and typed RPC client code from
source annotations.

```sh
moon run cmd -- agents <package-dir>
```

**What it generates:**

| File | Contents |
|---|---|
| `golem_agents.mbt` | `fn init {}` block registering all agents, `AgentTypeDef` definitions, constructor decoding, `impl RawAgent` with method dispatch |
| `golem_derive.mbt` | `@schema.IntoSchema` / `@schema.FromSchema` impls for `#derive.golem_schema` types; `@multimodal.MultimodalModality` impls for `#derive.multimodal` enums |
| `golem_clients.mbt` | RPC client structs (`<AgentName>Client`) with awaited, fire-and-forget (`trigger_*`), and scheduled (`schedule_*`) method variants |
| `golem_tools.mbt` | Tool descriptors, error schemas, registration, canonical-input decoding, and command dispatch |
| `golem_tool_clients.mbt` | Typed tool clients (`<ToolName>Client`) and nested clients for subcommand trees |

It also auto-adds the required imports to the target `moon.pkg` (e.g. `agents`, `schema`,
`schema_model`, `tool`, `interface/golem/agent/common`, `interface/golem/tool/common`,
`interface/golem/core/types`, `rpc`, `multimodal`, and `config`). The `reexports` subcommand
additionally adds the `gen` import.

## Supported Annotations

| Annotation | Target | Purpose |
|---|---|---|
| `#derive.agent` | struct | Marks a struct as a Golem agent |
| `#derive.agent("ephemeral")` | struct | Marks an agent as ephemeral (stateless) |
| `#derive.golem_schema` | struct, enum | Generates serialization impls |
| `#derive.multimodal` | enum | Generates `@multimodal.MultimodalModality` trait impl |
| `#derive.prompt_hint("...")` | method | Adds a prompt hint to the method definition |
| `#derive.tool(...)` | empty struct | Defines a tool and optional wire name/version |
| `#derive.command(...)` | public tool method | Configures command name, aliases, subtree, and behavioral annotations |
| `#derive.arg(...)` | public tool method | Maps one method parameter to a global, positional, tail, option, flag, or stream |
| `#derive.constraint(...)` | public tool method | Adds argument-presence or `value-is` constraints |
| `#derive.result(...)` | public tool method | Declares result formatters and the default formatter |
| `#derive.error(...)` | error enum/suberror case | Declares error kind, exit code, and optional typed payload |
| `#derive.example(...)` | tool, command, or error case | Adds a documented invocation example |

Doc comments (`///`) on structs, constructors, and methods are extracted as descriptions in the generated `AgentType` metadata.

## Defining Tools

Use `#derive.tool` on an empty struct and implement commands as public static methods. Wire names
default to lower-kebab-case. A method whose wire name equals the tool name is the optional root
command; the remaining methods are subcommands. A tool without a root method is a pure dispatcher.

```moonbit nocheck
///|
enum SearchError {
  #derive.error(kind="usage-error", exit_code="2")
  InvalidPattern(reason~ : String)
  #derive.error(kind="runtime-error", exit_code="1")
  NoMatch
}

///|
#derive.tool("search", version="1.0.0")
struct Search {}

///|
#derive.arg("case_sensitive", name="case-sensitive", scope="global", short="i", kind="flag")
#derive.arg("pattern", scope="positional", regex="^.+$")
#derive.arg("files", scope="tail", kind="file", direction="input", accepts_stdio=true)
pub fn Search::search(
  case_sensitive : Bool,
  pattern : String,
  files : Array[@schema.Path],
  stdin : @streams.InputStream,
  stdout : @streams.OutputStream,
) -> Result[Array[String], SearchError] {
  // ...
}

///|
#derive.command(alias="r")
#derive.arg("format", scope="option", default="json")
#derive.constraint("requires_all", value_is="format=json")
#derive.result("human", formatter="json", default="human")
pub fn Search::render(format : String) -> Result[String, SearchError] {
  // ...
}
```

### Argument mapping

`#derive.arg` starts with the MoonBit source parameter name. `name` overrides its wire name and
`alias` adds accepted aliases. The most commonly used properties are:

- `scope`: `global`, `positional`, `tail`, or `option`; `kind="flag"` and
  `kind="count-flag"` define flags.
- `short`, `env`, `required`, `default`, `value_name`, `negatable`, and `optional_scalar`.
- `repeatable`: `repeated`, `delimited`, or `either`; delimiter-aware modes also require `delim`.
- tail controls: `min`, `max`, `separator`, `verbatim`, and `accepts_stdio`.
- refinements: `regex`, `min_length`, `max_length`, numeric `min`/`max`/`bounds`/`unit`, path
  `kind`/`direction`/`mime`, and URL `scheme`.

Without an explicit mapping, `Bool` is a flag, a final `Array[T]` is a tail positional, other
arrays and maps are repeatable options, and other values are positionals. Explicit annotations are
recommended whenever the command-line surface matters.

The exact qualified runtime types `@tool.Principal`, `@streams.InputStream`, and
`@streams.OutputStream` are hidden invocation parameters. Principal and output-stream parameters
are not exposed by generated clients; input streams are accepted as client inputs. Output streams
are returned either alone or paired with the command's typed result.

### Constraints and errors

Supported constraint kinds are `requires_all`, `all_or_none`, `requires_any`, `mutex_groups`,
`implies`, and `forbids`. References use canonical argument names. `value_is="name=literal"`
compares a typed scalar, list element, map value, or tail element after the descriptor resolves the
literal against that argument's schema.

Every case in a tool error enum or typed `suberror` needs `#derive.error(kind=...)`, where `kind` is
`usage-error` or `runtime-error`; `exit_code` defaults to `2` for usage errors and `1` for runtime
errors. A case may carry zero or one typed payload. The generator emits one reusable
`@tool.ToolErrorSchema` implementation per error type.

### Subcommand trees

A command can graft another tool definition as a subtree:

```moonbit nocheck
#derive.command(alias="rmt", subtree="Remote")
#derive.arg("verbose", scope="global", kind="count-flag")
pub fn Git::remote(verbose : UInt) -> Unit { ignore(verbose) }
```

Subtree commands return `Unit` and may only define globals. The referenced tool remains an internal
subtool rather than a separately discoverable tool. Its methods are exposed through a nested typed
client such as `GitRemoteClient`, reached from `GitClient::remote(...)`.

## Typed Tool Clients

For each discoverable tool, `golem_tool_clients.mbt` contains a `<ToolName>Client` whose method
signatures mirror the authored commands. Clients encode canonical inputs, invoke
`golem:tool/host`, decode typed results, preserve stdout, and return custom errors without casts:

```moonbit nocheck
let client = SearchClient::new()
let result : Result[Array[String], @tool.ToolError[SearchError]] =
  client.search(false, "needle", [], stdin)
client.drop()
```

Call `drop()` when the client is no longer needed. Generated files are replaced on every `agents`
run and must not be edited by hand.

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
