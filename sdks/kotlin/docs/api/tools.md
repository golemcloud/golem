# Tools

> Exposing a top-level function as a `golem:tool@0.1.0` tool (`@Tool`), and discovering / invoking
> tools registered by other components (`golem:tool/host@0.1.0`). **Status:** 🟡 Partial. Both
> directions speak the **Golem 1.6.0 tool ABI**, including streamed stdin/stdout/stderr and declared errors; the whole surface was verified
> on a real 1.6.0 executor (see Invocation). Tool middleware is documented in [tool-middleware.md](tool-middleware.md); tool reflection
> (`ToolHost.describeAll` / `describe`) is documented in [reflection.md](reflection.md#tool-reflection).

## Overview

A Golem tool is a CLI-shaped capability a component exports alongside its
[agent](agent-model.md) surface. There are two directions:

- **Exposing** a tool — annotate a **top-level function** with [`@Tool`](#tool-annotation)
  (`cloud.golem.annotations.Tool`). Tools are stateless, so they are not agent methods. Its
  positional parameters are derived 1:1 from the function's parameters, in declaration order, and
  its return value becomes the tool's result.
- **Consuming** tools — use [`ToolHost`](#discovery--toolhost) (`cloud.golem.runtime.ToolHost`) to
  discover the tools the agent has access to, and the [`ToolRpc`](#invocation--toolrpc) resource to invoke
  one.

For the SDK overview see [`../../README.md`](../../README.md).

## `@Tool` annotation

`cloud.golem.annotations.Tool` marks a top-level function as a tool. Its identity is the root command name
(`name`); scope is one tool = one root command (no subcommands; a tool with subcommands, options or flags is a [`@ToolDefinition`](tool-middleware.md#arguments) interface consumed through its generated client), whose positionals map 1:1
from the function's parameters in declaration order (`@Arg` on one of them fails the build). The KSP processor registers every `@Tool`
function (one package per component, shared with any `@Agent` classes; registering two tools under one name fails) and the generated tool
exports call that registration first. Parameter and return types use the same type mapping as
agent methods. Throwing `ToolException(tag, message)` returns a `tool-error`
(`invalid-tool-name`, `invalid-input`, `constraint-violation`, `invalid-result`; any other tag becomes `invalid-input`); any other exception is returned as `invalid-input` with its message; a non-empty
`command-path` is rejected with `invalid-command-path`. Declared errors are raised as `custom-error` (see [Declared errors](#declared-errors)).

```kotlin
@Tool(name = "kt-echo", description = "Echoes its input")
fun ktEcho(value: String): String = "kt:$value"

@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Tool(
    val name: String,
    val description: String = "",
    val stdin: Boolean = false,    // the tool reads a stdin stream (optional for the caller)
    val stdout: Boolean = false,   // the tool writes a stdout stream (optional for the caller)
    val stderr: Boolean = false,   // the tool writes a stderr stream (optional for the caller)
    val errors: Array<ToolErrorCase> = [],   // the errors the tool may raise (see below)
)

annotation class ToolErrorCase(
    val name: String,                                   // lowercase kebab-case, e.g. "not-found"
    val description: String = "",
    val payload: KClass<*> = Unit::class,               // type of the error's payload; Unit::class = none
    val usage: Boolean = false,                         // a caller mistake (usage error) rather than a runtime failure
    val exitCode: Int = 1,                              // 0..255
)

/** Documents a single positional parameter of a [Tool]-annotated function. */
@Target(AnnotationTarget.VALUE_PARAMETER)
@Retention(AnnotationRetention.RUNTIME)
annotation class Command(
    val description: String = ""
)
```

### Declared errors

A tool declares the failures a caller can tell apart with `@Tool(errors = [ToolErrorCase(...)])` and raises one by
throwing `ToolCustomError(name, payload)`. The declarations are part of the tool's command tree (visible through
[`ToolHost.describe`](reflection.md#tool-reflection)) and the caller receives a `ToolError.CustomError(name, payload)`.

```kotlin
data class Limit(val max: Int, val reason: String)

@Tool(
    name = "lookup",
    errors = [
        ToolErrorCase("not-found", "No such key", usage = true, exitCode = 4),
        ToolErrorCase("over-limit", payload = Limit::class),
    ],
)
fun lookup(key: String): String {
    if (key.isEmpty()) throw ToolCustomError("not-found")                       // payloadless: the payload is the empty tuple
    if (key.length > 8) throw ToolCustomError("over-limit", SchemaValue.Record(listOf(SchemaValue.S32(8), SchemaValue.Str("too long"))))
    return key
}
```

Naming an error the tool did not declare, passing a payload to a payloadless case or omitting one for a case that has a payload is the
tool's own bug and is returned as `invalid-result`. KSP rejects a case name that is not kebab-case or an `exitCode` outside 0..255.
Payloads are passed as a [`SchemaValue`](types.md) of the declared type.

## Discovery — `ToolHost`

`cloud.golem.runtime.ToolHost` reads the tools registered by other components in the
environment. A discovered tool is projected down to the fields the SDK reads without decoding the
full CLI-command-tree structure — the stable `lookupName` a `ToolRpc` is created with, the
canonical identity (`name`, the root command's name), the `version`, and the implementing component:

```kotlin
data class RegisteredTool(val lookupName: String, val name: String, val version: String, val implementedBy: ComponentId)

object ToolHost {
    /** Every tool the calling agent has access to in the current environment. */
    fun getAllTools(): List<RegisteredTool>

    /** Looks up a single registered tool by name, iff the calling agent has access to it. */
    fun getTool(name: String): RegisteredTool?

    /** The full declared command tree (`ToolInfo`) of every accessible tool; see reflection.md. */
    fun describeAll(): List<ToolInfo>

    /** The declared command tree of one tool, iff the calling agent has access to it. */
    fun describe(name: String): ToolInfo?
}
```

`ComponentId` is the same `golem:core/types@2.0.0` id used across the [Host API](host-api.md).

## Invocation — `ToolRpc`

`cloud.golem.runtime.ToolRpc` is a handle for invoking a tool registered elsewhere. Construct
it with the tool's lookup name, invoke it via one of the three forms below, and `close()` it when done
(the handle is not tied to Kotlin/Wasm GC). `input` may be any composite `TypedSchemaValue`. All three
forms can attach streams (see [Streamed stdin, stdout and stderr](#streamed-stdin-stdout-and-stderr)).

**Golem 1.6.0 changes (breaking):** the constructor uses `tool-rpc.create` and throws `ToolRpcException`
if the host refuses it (creating a handle for an unregistered name still succeeds; the error shows on
invoke); `ToolFutureInvokeResult.get()` now **waits** for the terminal result instead of returning `null`
while pending, and `subscribe()` is gone; `ToolInvocationResult.stdoutHandle` is gone (the result's stream
attachments must be absent); `ToolRpcError.RemoteToolError` now carries the decoded `ToolError`; and
`ToolRpcError` gains `Cancelled` and `ResourceExhausted`.

```kotlin
class ToolRpc(toolName: String) {
    /** Fire-and-forget. Returns null once durably admitted, or the ToolRpcError the host reported. Sends [stdin] (then closes it) before returning. */
    fun invoke(commandPath: List<String>, input: TypedSchemaValue, stdin: ByteArray? = null): ToolRpcError?

    /** Blocking. Returns the tool's result value, or an error; optionally sends stdin and captures stdout / stderr. */
    fun invokeAndAwait(
        commandPath: List<String>,
        input: TypedSchemaValue,
        stdin: ByteArray? = null,
        captureStdout: Boolean = false,
        captureStderr: Boolean = false,
    ): ToolInvokeResult

    /** Durably admits the call and returns a future to await later; attached streams are driven inside `get()`. */
    fun asyncInvokeAndAwait(
        commandPath: List<String>,
        input: TypedSchemaValue,
        stdin: ByteArray? = null,
        captureStdout: Boolean = false,
        captureStderr: Boolean = false,
    ): ToolFutureInvokeResult

    /** Releases the tool-rpc handle. */
    fun close()
}

/** A pending async invocation (golem:tool/host `future-invoke-result`). */
class ToolFutureInvokeResult {
    fun get(): ToolInvokeResult   // waits; repeated calls return the same terminal
    fun cancel()                  // explicitly cancels the operation
    fun close()                   // detaches observation; does not cancel accepted work

    companion object {
        /** Waits for several futures as one causal batch (`get-invoke-results`); results come back in input order. */
        fun getAll(futures: List<ToolFutureInvokeResult>): List<ToolInvokeResult>
    }
}

/** The awaited outcome: the tool's `invocation-result`, or an error. */
sealed class ToolInvokeResult {
    data class Ok(val value: ToolInvocationResult) : ToolInvokeResult()
    data class Err(val error: ToolRpcError) : ToolInvokeResult()
}

data class ToolInvocationResult(
    val result: TypedSchemaValue?,           // the tool's return value, fully decoded (self-describing)
    val stdout: ByteArray? = null,           // captured stdout (captureStdout = true)
    val stderr: ByteArray? = null,           // captured stderr (captureStderr = true)
    val stdoutFailure: ToolStreamFailure? = null, // set if the tool ended stdout with a failure
    val stderrFailure: ToolStreamFailure? = null,
)

sealed class ToolRpcError {
    data class ProtocolError(val message: String) : ToolRpcError()
    data class Denied(val message: String) : ToolRpcError()
    data class NotFound(val message: String) : ToolRpcError()
    data class RemoteInternalError(val message: String) : ToolRpcError()
    data class RemoteToolError(val error: ToolError) : ToolRpcError()
    object Cancelled : ToolRpcError()
    data class ResourceExhausted(val message: String) : ToolRpcError()
}

sealed class ToolError {
    data class InvalidToolName(val message: String) : ToolError()
    data class InvalidCommandPath(val path: List<String>) : ToolError()
    data class InvalidInput(val message: String) : ToolError()
    data class ConstraintViolation(val message: String) : ToolError()
    data class InvalidResult(val message: String) : ToolError()
    data class CustomError(val name: String, val payload: TypedSchemaValue) : ToolError()
}
```

The calls added after that verification (`invoke` with stdin, `asyncInvokeAndAwait` with streams, `ToolFutureInvokeResult.getAll`, declared errors) ran on a real executor: `@Tool(errors = …)` returned `not-found` (payloadless) and `over-limit` (record payload) as `custom-error` values, `asyncInvokeAndAwait` with stdin and captured stdout/stderr returned `HELLO` / `done:5` / `5`, `getAll` returned a success and a declared error in input order, and `invoke` with stdin was admitted. Verified earlier on a real 1.6.0 executor against a
Kotlin provider and the Rust `tool-streaming` provider: `getAllTools`, `getTool` (and `null` for an unknown
name), repeated `invokeAndAwait`, `asyncInvokeAndAwait` + `get` (twice, same terminal), `invoke`, and a
`remote-tool-error(invalid-command-path)`.

## Streamed stdin, stdout and stderr

A tool command may declare optional `stdin`, `stdout` and `stderr` byte streams.

**Providing them** — put the flags on `@Tool` and take a `ToolStreams` parameter. It is not a positional;
it receives the invocation's streams (each is `null` when the caller did not attach it):

```kotlin
@Tool(name = "kt-upper", description = "Uppercases stdin", stdin = true, stdout = true, stderr = true)
fun ktUpper(mode: String, io: ToolStreams): Int {
    var total = 0
    while (true) {
        val chunk = io.stdin?.read() ?: break          // null at the clean end of input
        total += chunk.size
        io.stdout?.write(chunk.decodeToString().uppercase().encodeToByteArray())
    }
    io.stderr?.write("done:$total".encodeToByteArray())
    return total
}
```

`ToolInput.read()` returns the next non-empty chunk, `null` at end of input, and throws `ToolStreamException` (carrying a
`ToolStreamFailure`: `Cancelled`, `Abandoned`, `ResourceExhausted` or `Failed(message)`) if
the caller ended stdin with a failure (`readAll(maxBytes = 16 MiB)` collects everything and throws `ToolStreamException(ResourceExhausted)` past `maxBytes`; captured stdout/stderr of a nested call are cut off at 16 MiB each and reported as `ResourceExhausted`; the capture's readable end is released at the cutoff, so a tool that writes more is not held up and the call still completes). `ToolOutput.write(bytes)` returns `false`
once the caller stopped reading; `finish()` ends the stream cleanly and `fail(ToolStreamFailure)` ends it with a
recoverable failure (the first terminal wins). When the body returns normally, any writer it left open is finished;
if it throws, the writers are dropped and the caller sees `abandoned`. The declared specs are optional
(`required = false`, `application/octet-stream`).

**Consuming them** — pass `stdin` bytes and/or `captureStdout` / `captureStderr` to `ToolRpc.invokeAndAwait`:

```kotlin
val r = rpc.invokeAndAwait(emptyList(), input, stdin = "hello".encodeToByteArray(), captureStdout = true, captureStderr = true)
if (r is ToolInvokeResult.Ok) println(r.value.stdout?.decodeToString())
```

The host requires the caller to drive every attached stream concurrently with the call, so the SDK admits the call
with `async-invoke-and-await` and runs a single event loop over the stdin write and the output reads before it awaits
the result; a tool that echoes its input cannot deadlock the caller. Verified on a real 1.6.0 executor in both
directions (Kotlin caller ↔ Rust `tool-streaming` provider, Kotlin caller ↔ Kotlin provider): stdin→stdout echo, a
300 000-byte round trip with byte-exact output, stdout plus stderr, an absent stdin, an unattached stderr, and a
tool error after partial output. **Limit:** captured and read data is held in the bump allocator (16 MB per
invocation, never freed within one), so streams of many megabytes are not supported yet.

### Awaiting a tool's result

```kotlin
val rpc = ToolRpc("formatter")
try {
    when (val r = rpc.invokeAndAwait(listOf("format"), TypedSchemaValue("record<source:string>", SchemaValue.Record(listOf(SchemaValue.Str(src)))))) {
        is ToolInvokeResult.Ok  -> (r.value.result?.value as? SchemaValue.Str)?.v   // the formatted output
        is ToolInvokeResult.Err -> error("format failed: ${r.error}")
    }
} finally {
    rpc.close()
}
```

`input` is a [`TypedSchemaValue`](types.md) — a self-describing value pairing a WIT type string
with a matching `SchemaValue`; its root must be a record with one field per positional/option/flag of
the selected command. It supports the **full composite grammar** (primitives plus
records, lists, options, variants, enums, tuples, maps, results — arbitrarily nested):

```kotlin
data class TypedSchemaValue(val witType: String, val value: SchemaValue)
```

## Examples

Exposing a function as a tool:

```kotlin
import cloud.golem.annotations.Tool

@Tool(name = "greet", description = "Print a greeting for someone")
fun greet(name: String, times: Int): String = (1..times).joinToString("\n") { "hello, $name" }
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
    val rpc = ToolRpc(tool.lookupName)
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

- **What works today:** exposing tools via `@Tool`, discovery via `ToolHost.getAllTools` / `getTool`,
  and all three invocation forms — fire-and-forget `invoke`, blocking `invokeAndAwait` and
  `asyncInvokeAndAwait` (+ `ToolFutureInvokeResult.get`). Results decode fully:
  `invocation-result.result` is a self-describing `typed-schema-value`, lifted with no prior type
  knowledge, and a failed call carries the decoded `ToolError`. The async host calls are
  imported async-lowered and waited out inside the SDK (`awaitSubtask`), so the exported functions
  stay plain synchronous exports.
- **Follow-ups:** none for stream size: tool streams are read and written in 64 KiB chunks, and the buffered paths (`readAll`, captured stdout and stderr) stop at 16 MiB. A whole argument or result is lowered through the arena at its own size and is bounded by the agent's memory limit. `@Tool` functions stay single-command; subcommands are declared with a `@ToolDefinition` interface. Streams attached to `asyncInvokeAndAwait` are driven while `get()` / `getAll` run, not in the background.
- **`RegisteredTool` is a lossy projection.** It keeps the WIT `tool` record's
  lookup name, canonical identity (`name` = `commands.nodes[0].name`), `version`, and
  `implementedBy`. For the decoded command tree (subcommands, options, flags, positionals, stream flags, declared errors) use
  `ToolHost.describeAll()` / `describe(name)` ([reflection.md](reflection.md#tool-reflection)); each argument carries its schema-graph index and a resolved `witType` string.
- Close each `ToolRpc` handle when done.

## Typed tool definitions

Describe a tool as an interface instead of building input records by hand. Each method is a command; its parameters are positionals in declaration order, a `Principal` parameter is injected
(never a positional), and the return type is `T`, `Unit` or `Either<E, T>` with `E` a sealed class of declared errors (`@ToolError(kind, exitCode, description)` on each case; the error name is the case's
class name in kebab-case; the payload is the case's property when it has one, a record of the properties when it has several, none when it has none). A method with no parameters that returns another `@ToolDefinition` interface is a nested command group.

```kotlin
@ToolDefinition(name = "public-echo", version = "1.2.3")
interface PublicEcho { fun echo(value: String): Either<PublicError, String> }

val client = PublicEchoClient(ToolRpc("public-echo"))
when (val r = client.echo("hi")) {
    is Either.Right -> println(r.value)
    is Either.Left -> when (val e = r.value) {
        is ToolInvokeError.Tool -> println("rejected: ${e.error}")
        is ToolInvokeError.Cancelled -> println("cancelled")
        else -> println("protocol failure: $e")
    }
}
```

`ToolInvokeError<E>` has `Tool(error)` for a declared failure and `InvalidToolName`, `InvalidCommandPath`, `InvalidInput`, `ConstraintViolation`, `InvalidResult`, `Cancelled`, `ResourceExhausted` and `UnknownCustomError(name, payload)` (a custom error the definition does not declare) for the rest;
`mapTool { }` converts only the declared error. A `Unit` command returns `Right(Unit)`. The client is awaited (one call at a time), and the same definition also yields typed
[tool middleware](tool-middleware.md#typed-per-tool-middleware). The interface may only hold abstract, non-suspend, non-generic functions (no properties, default bodies or
super-interfaces), parameters must not start with `golem` or be `Unit`, and a case without `@ToolError` is a runtime error with exit code 1. The wrapped-tool handle (`<Name>Underlying`) takes
no `Principal`: the host supplies the caller's principal. Parameters are positionals unless annotated with `@Arg` (options, flags, tails, constraints, group globals, streams: see [tool-middleware.md](tool-middleware.md#arguments)).

### Serving a tool from the interface

A class annotated `@ToolImplementation` serves the tool a `@ToolDefinition` interface describes. It has a public no-argument constructor and implements exactly one `@ToolDefinition` interface; a method that opens a command group returns the object that implements the group's interface. The runtime creates a fresh instance for each invocation, so keep no state in it. The host calls `discover-tools` / `get-tool` with the interface's command tree, and `invoke` selects the command by `command-path`.

```kotlin
@ToolImplementation
class MatrixCoreToolImpl : MatrixCoreTool {
    override fun artifact(): MatrixArtifactTool = MatrixArtifactToolImpl()
}

class MatrixArtifactToolImpl : MatrixArtifactTool {
    override fun inspect(request: MatrixRequest, multiplier: Long, principal: Principal): Either<MatrixError, MatrixResult> =
        Either.Right(MatrixResult(/* ... */, principal = if (principal is Principal.Oidc) "oidc:${principal.sub}" else "other"))
}
```

A `Principal` parameter receives the principal of the call as the host supplied it. Returning `Either.Left(case)` raises the declared error `case`. A command that reads stdin or writes stdout or stderr is rejected at build time: a provider does not support streams yet. A tool name is served by one registration, either a `@Tool` function or a `@ToolImplementation` class.

See also: [Agent model](agent-model.md) · [Types](types.md) · [WASI](wasi.md) ·
[Host API](host-api.md) · [Tool middleware](tool-middleware.md) · [Reflection](reflection.md) · [SDK README](../../README.md).
