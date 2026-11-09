# Agent reflection and dynamic clients

> Read the agent types registered with the Golem host and call any agent by method name, without a generated client. **Status:** 🟡 Partial — agent reflection,
> dynamic agent clients and tool command-tree reflection are done; tool argument types are exposed as witType strings (plus the schema-graph index), and
> defaults, constraints, formatters and examples are decoded. Verified on a real 1.6.0 executor.

`AgentReflection`, `AgentTypeInfo`, `DynamicAgentClient`, `ToolHost` and friends live in `cloud.golem.runtime`.

```kotlin
val type = AgentReflection.agentType("Counter")!!
println(type.methods.map { "${it.name}(${it.parameters.joinToString { p -> "${p.name}: ${p.witType}" }}) -> ${it.resultWitType ?: "()"}" })
// [inc(by: s32) -> s32, move(from: record<x:s32,y:s32>, steps: list<s32>) -> record<x:s32,y:s32>, label() -> string, reset() -> ()]

val client = AgentReflection.dynamicClient("Counter(\"c1\")")!!            // an agent-id string (see HostApi.makeAgentId)
val result = client.invoke("inc", listOf(SchemaValue.S32(5)))                // positional
client.invoke("inc", mapOf("by" to SchemaValue.S32(7)))                       // by parameter name
client.close()
```

- `AgentReflection.agentTypes()` / `agentType(name)` return `AgentTypeInfo`: `typeName`, `description`, `kind` (`regular` | `http-router`), `mode` (`durable` | `ephemeral`), `sourceLanguage`,
  `constructorParameters`, `methods` and `implementedBy` (a `ComponentId`); `AgentTypeInfo.method(name)` looks a method up. Each `AgentMethodInfo` has `name`, `description`, `parameters`, `resultWitType` (`null` = no result) `readOnly` and `userParameters` (the non-injected parameters); each
  `AgentParameterInfo` has `name`, `witType` and `autoInjected` (the caller's principal is injected by the runtime and is not passed).
- Types are the SDK's **witType strings** (`s32`, `record<x:s32,y:s32>`, `list<s32>`, `option<...>`, ...), the same grammar generated clients use, so arguments and results are plain
  [`SchemaValue`](types.md)s. A recursive type cannot be written as a flat witType and is reported as an error.
- `AgentReflection.dynamicClient(agentId)` parses the id (its constructor arguments come from the id), looks up the type and opens an RPC client; it returns `null` if the id does not parse or the type is
  not registered, and throws `RpcException` if the host refuses the client. `DynamicAgentClient.invoke(method, arguments)` (a `List<SchemaValue>`, default empty, or a `Map<String, SchemaValue>` by name) checks the method and the argument count / names against the declaration
  (`DynamicCallException`, also thrown for unknown or missing named arguments) and returns the same `RpcResult` as [`WasmRpc`](rpc.md), decoded with the declared result type. `close()` releases the client.
- Dynamic calls go through the host's normal RPC path, so the underlying `WasmRpc` supports permission cards (`scopeCard`) and config overrides, but `DynamicAgentClient` does not expose them (it calls `invokeAndAwait` with the default scope card and no overrides).

Verified: a Kotlin agent listed every registered type, described `Counter` (constructor, four methods with record / list parameters and a unit result, `kind = regular`, `mode = durable`, its description),
and called it dynamically: `inc` 5 then 7 (stateful, 12), a record + list call, a no-result call, and the three argument errors (unknown method, wrong count, missing named argument).

## Tool reflection

`ToolHost.describeAll()` / `ToolHost.describe(name)` return a `ToolInfo` for each tool the calling agent may use: the `RegisteredTool`, `requiresFilesystem` and
the decoded command tree (`commands`, node 0 is the root). Each `ToolCommandInfo` has `name`, `aliases`, `summary`, `subcommands` (indices), `globals` and an optional `body`
with its `arguments` (`ToolArgumentInfo`: kind `POSITIONAL` / `TAIL` / `OPTION` / `FLAG`, name, aliases, short option character (a Unicode scalar, `Int?`), required, summary, type node index and `witType`), `stdin` / `stdout` / `stderr`
presence, `hasResult`, declared error names and annotations. `ToolInfo.find(path)` resolves a command path (aliases accepted) and `runnablePaths()` lists every command that has a body.

Each argument's `typeIndex` is its node in the tool's schema graph, and `witType` is that type resolved to the same witType string grammar as agent reflection (`string`, `list<s32>`, `record<...>`), so values are plain [`SchemaValue`](types.md)s; `witType` is `null` for flags and for a recursive type that cannot be written flat. `ToolArgumentInfo` also has `shape` (`FIXED`, `TAIL`, `SCALAR`, `OPTIONAL_SCALAR`, `REPEATABLE_LIST`, `REPEATABLE_MAP`, `BOOL_FLAG`, `COUNT_FLAG`), `default` (a `SchemaValue` interpreted against `witType`), `env`, `valueName`, `negatable` / `flagDefault` (bool flags), `countMax` (count flags), `tailMin` / `tailMax` / `separator` (tails), and the restrictions of the argument's type: `min` / `max` (numeric bounds; integers in plain digits, floats as `Double.toString()`) and `minLength` / `maxLength` (`Long`, in Unicode scalar values) / `regex` (text). `ToolBodyInfo` has `constraints` (`ToolConstraint`: `RequiresAll`, `RequiresAny`, `AllOrNone`, `MutexGroups`, `Implies`, `Forbids` over `ToolRef(name, value?)`), `formatters` and `defaultFormatter`; `ToolCommandInfo.examples` lists `ToolExample(title, body)`. The decoders are round-trip tested against the lowering of `@ToolDefinition` metadata.

Verified on a real 1.6.0 executor against the Rust provider components (`scripts/tool-reflection/`): nested subcommands (`artifact/inspect`), tail positionals, flags, declared errors and
stream flags (`stdin` + `stdout` for `run-capable`, `stdout` + `stderr` for `dual-pressure`) decoded correctly; an unknown tool returned `null`.

See also: [RPC](rpc.md) · [Host API](host-api.md) · [SDK README](../../README.md).
