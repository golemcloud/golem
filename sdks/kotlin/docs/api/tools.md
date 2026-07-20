# Tools

> Exposing an agent method as a `golem:tool@0.1.0` tool (`@Tool`), and discovering / invoking
> tools registered by other components (`golem:tool/host@0.1.0`). **Status:** 🟡 Partial —
> exposing tools, discovery (`getAllTools`/`getTool`), and all three invocation forms
> (fire-and-forget `invoke`, blocking `invokeAndAwait`, async `asyncInvokeAndAwait`) are done;
> only **streamed stdin** remains a follow-up.

## Overview

A Golem tool is a CLI-shaped capability a component exports alongside its
[agent](agent-model.md) surface. There are two directions:

- **Exposing** a tool — annotate an `@Agent` method with [`@Tool`](#tool-annotation)
  (`cloud.golem.annotations.Tool`). Its positional parameters are derived 1:1 from the method's
  parameters, in declaration order.
- **Consuming** tools — use [`ToolHost`](#toolhost) (`cloud.golem.runtime.ToolHost`) to
  discover the tools the agent has access to, and the [`ToolRpc`](#toolrpc) resource to invoke
  one.

For the SDK overview see [`../../README.md`](../../README.md).

## `@Tool` annotation

`cloud.golem.annotations.Tool` marks a method as a tool. Its identity is the root command name
(`name`); MVP scope is one tool = one root command (no subcommands), whose positionals map 1:1
from the method's parameters in declaration order. `@Command` documents an individual
positional parameter.

```kotlin
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Tool(
    val name: String,
    val description: String = ""
)

@Target(AnnotationTarget.VALUE_PARAMETER)
@Retention(AnnotationRetention.RUNTIME)
annotation class Command(
    val description: String = ""
)
```

## Discovery — `ToolHost`

`cloud.golem.runtime.ToolHost` reads the tools registered by other components in the
environment. A discovered tool is projected down to the fields the SDK can currently read
without decoding the full CLI-command-tree structure — its canonical identity (`name`), its
`version`, and the component that implements it:

```kotlin
data class RegisteredTool(val name: String, val version: String, val implementedBy: ComponentId)

object ToolHost {
    /** Every tool the calling agent has access to in the current environment. */
    fun getAllTools(): List<RegisteredTool>

    /** Looks up a single registered tool by name, iff the calling agent has access to it. */
    fun getTool(name: String): RegisteredTool?
}
```

`ComponentId` is the same `golem:core/types@2.0.0` id used across the [Host API](host-api.md).

## Invocation — `ToolRpc`

`cloud.golem.runtime.ToolRpc` is a handle for invoking a tool registered elsewhere. Construct
it with the tool's name, invoke it via one of the three forms below, and `close()` it when done
(the handle is not tied to Kotlin/Wasm GC). `stdin` is always `none` on every form (streamed
stdin is the remaining follow-up); `input` may be any composite `TypedSchemaValue`.

```kotlin
class ToolRpc(toolName: String) {
    /** Fire-and-forget. Returns null on success, or the ToolRpcError the host reported. */
    fun invoke(commandPath: List<String>, input: TypedSchemaValue): ToolRpcError?

    /** Blocking. Returns the tool's result value + optional stdout stream, or an error. */
    fun invokeAndAwait(commandPath: List<String>, input: TypedSchemaValue): ToolInvokeResult

    /** Async. Returns a future to poll (get) or wait on (subscribe). */
    fun asyncInvokeAndAwait(commandPath: List<String>, input: TypedSchemaValue): ToolFutureInvokeResult

    /** Releases the tool-rpc handle. */
    fun close()
}

/** A pending async invocation (golem:tool/host `future-invoke-result`). */
class ToolFutureInvokeResult {
    fun subscribe(): Int           // wasi:io/poll pollable handle (caller-owned)
    fun get(): ToolInvokeResult?   // null while pending
    fun cancel()
    fun close()
}

/** The awaited outcome: the tool's `invocation-result`, or an error. */
sealed class ToolInvokeResult {
    data class Ok(val value: ToolInvocationResult) : ToolInvokeResult()
    data class Err(val error: ToolRpcError) : ToolInvokeResult()
}

data class ToolInvocationResult(
    val result: TypedSchemaValue?, // the tool's return value, fully decoded (self-describing)
    val stdoutHandle: Int?,        // opaque wasi:io output-stream handle for stdout, or null
)

sealed class ToolRpcError {
    data class ProtocolError(val message: String) : ToolRpcError()
    data class Denied(val message: String) : ToolRpcError()
    data class NotFound(val message: String) : ToolRpcError()
    data class RemoteInternalError(val message: String) : ToolRpcError()
    /** `remote-tool-error(tool-error)` — the nested tool-error payload is not yet decoded. */
    object RemoteToolError : ToolRpcError()
}
```

### Awaiting a tool's result

```kotlin
val rpc = ToolRpc("formatter")
try {
    when (val r = rpc.invokeAndAwait(listOf("format"), TypedSchemaValue("string", SchemaValue.Str(src)))) {
        is ToolInvokeResult.Ok  -> (r.value.result?.value as? SchemaValue.Str)?.v   // the formatted output
        is ToolInvokeResult.Err -> error("format failed: ${r.error}")
    }
} finally {
    rpc.close()
}
```

`input` is a [`TypedSchemaValue`](types.md) — a self-describing value pairing a WIT type string
with a matching `SchemaValue`. It supports the **full composite grammar** (primitives plus
records, lists, options, variants, enums, tuples, maps, results — arbitrarily nested):

```kotlin
data class TypedSchemaValue(val witType: String, val value: SchemaValue)
```

## Examples

Exposing a method as a tool:

```kotlin
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Command
import cloud.golem.annotations.Tool

@Agent
class GreeterAgent {

    @Tool(name = "greet", description = "Print a greeting for someone")
    fun greet(
        @Command(description = "the name to greet") name: String,
        @Command(description = "how many times") times: Int
    ): String = (1..times).joinToString("\n") { "hello, $name" }
}
```

Discovering the tools available to the current agent:

```kotlin
import cloud.golem.runtime.ToolHost

fun listTools(): List<String> =
    ToolHost.getAllTools().map { "${it.name}@${it.version}" }
```

Invoking a tool (fire-and-forget). The `input` here is a simple `string`, but any composite
`TypedSchemaValue` (e.g. `TypedSchemaValue("record<path:string,check:bool>", SchemaValue.Record(…))`)
works the same way:

```kotlin
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.ToolHost
import cloud.golem.runtime.ToolRpc
import cloud.golem.runtime.ToolRpcError
import cloud.golem.runtime.TypedSchemaValue

fun runFormatter(path: String): String? {
    val tool = ToolHost.getTool("formatter") ?: return "no such tool"
    val rpc = ToolRpc(tool.name)
    try {
        val input = TypedSchemaValue("string", SchemaValue.Str(path))
        return when (val err = rpc.invoke(commandPath = listOf("format"), input = input)) {
            null -> null // success
            is ToolRpcError.NotFound -> "command not found: ${err.message}"
            is ToolRpcError.Denied -> "denied: ${err.message}"
            else -> "invocation failed: $err"
        }
    } finally {
        rpc.close()
    }
}
```

## Notes

- **What works today:** exposing tools via `@Tool`, discovery via `ToolHost.getAllTools` /
  `getTool`, and all three invocation forms — fire-and-forget `invoke` (`result<_, rpc-error>`),
  blocking `invokeAndAwait` (`result<invocation-result, rpc-error>`), and async
  `asyncInvokeAndAwait` (`future-invoke-result`). Results decode fully: `invocation-result.result`
  is a self-describing `typed-schema-value`, lifted with no prior type knowledge. `input` payloads
  support the full composite grammar.
- **Follow-ups:** streamed **stdin** — every invoke form passes `stdin = none`; supplying a real
  `wasi:io/streams` input-stream needs stream-construction plumbing the SDK doesn't have yet.
  `ToolInvocationResult.stdoutHandle` is surfaced as an opaque `wasi:io` output-stream handle
  (stream reads not yet wrapped). `ToolRpcError.RemoteToolError`'s nested `tool-error` is still
  undecoded.
- **Discovery is lossy by design.** `RegisteredTool` projects the WIT `tool` record down to its
  canonical identity field (`name` = `commands.nodes[0].name`), `version`, and `implementedBy`.
  The full CLI-command-tree (options, flags, positionals, constraints) is not yet decoded.
- **`ToolRpcError.RemoteToolError`** currently carries no payload — the nested `tool-error` is
  not yet decoded.
- Close each `ToolRpc` handle when done.

See also: [Agent model](agent-model.md) · [Types](types.md) · [WASI](wasi.md) ·
[Host API](host-api.md) · [SDK README](../../README.md).
