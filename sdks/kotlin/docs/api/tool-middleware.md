# Tool middleware

> Wrap tool invocations: a middleware sees the tool name, command path and typed input of a call, may call the wrapped tool, transform its result and
> output, or reject the call. **Status:** 🟡 Partial — **universal** middleware (wraps any tool) and **typed per-tool** middleware (transparent and adapter, generated from a
> `@ToolDefinition` interface) are verified on a real executor (see below). Options, flags, tails, constraints, group globals, stdin/stdout/stderr, overlapping calls, cancellation and output forwarding are
> implemented and verified on a real executor (see "Arguments" and "Streams, overlap and cancellation"); crash replay of an outstanding wrapped call has not been run.
> Installation parameters are supported for both (see below).

```kotlin
@ToolMiddleware(name = "upper-stdout", description = "Upper-cases the tool's stdout")
class UpperStdout : UniversalToolMiddleware {
    override fun invoke(invocation: ToolMiddlewareInvocation, underlying: ToolUnderlying): ToolMiddlewareOutcome =
        when (val outcome = underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)) {
            is UnderlyingOutcome.Ok -> ToolMiddlewareOutcome.Ok(
                outcome.value.result,
                outcome.value.stdout?.decodeToString()?.uppercase()?.encodeToByteArray(),
                outcome.value.stderr,
            )
            else -> ToolMiddlewareOutcome.passThrough(outcome)
        }
}

@ToolMiddleware(name = "deny")
class Deny : UniversalToolMiddleware {
    override fun invoke(invocation: ToolMiddlewareInvocation, underlying: ToolUnderlying) =
        ToolMiddlewareOutcome.Err(ToolError.ConstraintViolation("denied: ${invocation.toolName} ${invocation.commandPath}"))
}
```

- **Registration**: annotate a class with a public no-argument constructor that implements `UniversalToolMiddleware` with `@ToolMiddleware(name, version = "0.1.0", aliases = [], description = "")`.
  `aliases` are alternative names the middleware answers to; they reach `tool-middleware.aliases`. A middleware also needs a manifest declaration —
  `tools: { middleware: { <name>: { component: <component> } } }` — or the deploy refuses the component with `has no matching top-level declaration`.
  The runtime creates a fresh instance for every top-level invocation, so keep no state in it. The component exports `golem:tool/tool-middleware-guest`
  (`discover-tool-middlewares`, `get-tool-middleware`, `invoke-tool-middleware`); the definitions are `universal` scope; the parameter schema is empty unless `@ToolMiddleware(parameters = Cfg::class)` is given.
- **`ToolMiddlewareInvocation`**: `middlewareName`, `toolName`, `parameters` (the installation's static configuration, self-describing; an empty record unless declared) and `parametersAs<Cfg>()` (the same value decoded to the declared class), `commandPath`, `input` (typed), `stdin`
  (a `ToolInput?`: the caller's stdin, if the command declares one) and `principal` (`Principal`).
- **`ToolUnderlying`** is the next layer. `invoke(commandPath, input, stdin)` calls it and waits for the result: `UnderlyingOutcome.Ok(ToolUnderlyingResult(result, stdout, stderr))` or
  `UnderlyingOutcome.Err(UnderlyingError)` (`ToolFailure(toolError)`, `ProtocolError`, `Denied`, `InternalError`, `Cancelled`, `ResourceExhausted`). Pass `invocation.stdin` to forward the
  caller's stdin to the wrapped tool (it can be forwarded once). The underlying is valid only during the middleware invocation; the runtime revokes it when the handler returns.
- **Outputs are buffered by default**: the host needs the wrapped tool's stdout and stderr read concurrently with the structured result, so `invoke` runs one event loop over the result and both streams
  and returns whole byte arrays. The middleware returns the bytes it wants written to the caller's stdout / stderr (`ToolMiddlewareOutcome.Ok(result, stdout, stderr)`; `null` = nothing). Pass the caller's writers to `ToolUnderlying.start(…).forwardOutputs(stdout, stderr)` (`ToolMiddlewareInvocation.stdout` / `stderr`) to write the wrapped tool's output as it arrives instead. Output beyond 16 MiB ends the call with `ResourceExhausted`.
- **Results**: `ToolMiddlewareOutcome.Ok(...)`, `.Err(ToolError)` (`InvalidToolName`, `InvalidCommandPath`, `InvalidInput`, `ConstraintViolation`, `InvalidResult`, `CustomError`), or
  `ToolMiddlewareOutcome.passThrough(outcome)` to forward a wrapped call's result and errors. An exception thrown by the middleware becomes `invalid-input` (a `ToolException` a
  `constraint-violation`, whatever its tag).

## Verification

Run on the 1.6.0 executor with a Kotlin component holding three middleware (`scripts/tool-middleware/ToolMiddlewares.kt`) installed on the Rust `streaming` tool and a Kotlin caller: the host discovered all three
definitions; the **pass-through** returned results identical to the direct calls, including forwarded stdin (a 300 000-byte echo came back byte-exact); the **upper-stdout** transform changed the stdout
(`HELLO STDIN`, `MARKER:`) and kept stderr; **deny** rejected every call with `constraint-violation` carrying the tool name and command path.

**Not covered:** crash replay of an outstanding wrapped call.

See also: [Tools](tools.md) · [Streams](streams.md) · [SDK README](../../README.md).

## Typed per-tool middleware

Describe a tool once as an annotated interface (see [Typed tool definitions](tools.md#typed-tool-definitions)); KSP generates, in the interface's package, `XDefinition`, `XClient`,
`XUnderlying`, `XMiddleware` and `XAdapter<U>`. Two middlewares with one name fail registration, and the runtime lists middlewares sorted by name. A subclass annotated `@ToolMiddleware` registers a **monomorphic** definition: the host scopes it to one tool.

```kotlin
sealed class PublicError {
    @ToolError(kind = ToolErrorKind.RUNTIME, exitCode = 1) data class Rejected(val message: String) : PublicError()
}
sealed class BackendError {
    @ToolError(kind = ToolErrorKind.RUNTIME, exitCode = 1) data class Failed(val message: String) : BackendError()
}

@ToolDefinition(name = "public-echo", version = "1.2.3")
interface PublicEcho { fun echo(value: String): Either<PublicError, String> }

@ToolDefinition(name = "backend-echo", version = "4.5.6")
interface BackendEcho { fun execute(encoded: String): Either<BackendError, Long> }

// Transparent: presents and wraps the same tool; commands it does not override forward to the wrapped tool.
@ToolMiddleware(name = "audit-echo")
class AuditEcho : PublicEchoMiddleware() {
    override fun echo(underlying: PublicEchoUnderlying, value: String) = underlying.echo(value)
}

// Adapter: presents PublicEcho while wrapping BackendEcho; it must override every presented command.
data class Prefix(val text: String)

@ToolMiddleware(name = "backend-echo-adapter", parameters = Prefix::class)
class EchoAdapter : PublicEchoAdapter<BackendEchoUnderlying>() {
    override fun echo(underlying: BackendEchoUnderlying, value: String): Either<ToolInvokeError<PublicError>, String> =
        when (val r = underlying.execute("${parameters<Prefix>().text}:$value").mapTool { PublicError.Rejected(it.toString()) }) {
            is Either.Right -> Either.Right(r.value.toString())
            is Either.Left -> Either.Left(r.value)
        }
}
```

- **Scope**: a transparent middleware registers `scope = monomorphic { presented = X, expected = none }`; an adapter registers `expected = Y`. The host does not require byte equality with the real tool; it
  compiles value projections between the two and checks `expected` against the real tool at install.
- **Handlers** receive the typed wrapped tool as their first parameter, then the arguments of the enclosing command groups, then the command's own parameters in declaration order. A `Principal` parameter in the interface is passed from the invocation
  (it is never in the input record); the typed wrapped-tool handle takes none, because the host supplies the principal of the call. A decode failure of the input is `invalid-input`;
  the dispatcher does not catch an exception thrown by the handler. A command inside a nested group is a handler named by joining the path in camel case (`reports` → `low` becomes `reportsLow`).
- **`parameters<P>()`** returns the installation parameters decoded to the type given in `@ToolMiddleware(parameters = P::class)`; call it inside a handler.
- **Errors**: a handler returns `Either<ToolInvokeError<E>, T>`. `Right(v)` is the result; `Left(Tool(e))` becomes the declared `custom-error`; `InvalidToolName`, `InvalidCommandPath`, `InvalidInput`,
  `ConstraintViolation` and `InvalidResult` become the same `tool-error` case; `Cancelled` and `ResourceExhausted` become `constraint-violation`. A wrapped call's failure is typed the same way in reverse:
  `ToolFailure(custom-error)` becomes `Tool(decoded)`, `ProtocolError` / `Denied` become `InvalidInput`, `InternalError` becomes `InvalidResult`. An error name the definition does not declare becomes
  `UnknownCustomError(name, payload)`, which a middleware returns to its caller as the same `custom-error`. A wrong-length or wrong-typed input record becomes `InvalidInput`, an unknown command path `InvalidCommandPath`; neither traps. A handler that throws is reported by the middleware runtime exactly as for a universal middleware
  (`ToolException` becomes `constraint-violation`, anything else `invalid-input`), and the wrapped tool is still revoked.
- **Cross-module use**: the interface, its error classes and the generated types are public, so a library can declare them and another module can write the middleware or call the client. The consuming
  module needs the library and the KSP processor; KSP rebuilds the tool model from the library's compiled declaration. The generated types are found by name (`<Tool>Middleware` → `<Tool>`, `<Tool>Underlying` → `<Tool>`).
- **Compile-time checks** (KSP errors, prefixed `GolemKSP:`): two methods that kebab-case to the same command name, an error class with no cases or two cases with one name, an unsupported parameter type, a
  positional parameter of a command group, an invalid `@Arg` (see below), an adapter that does not override a presented command, an adapter whose type argument is not a generated `…Underlying`, an adapter whose presented and expected tools declare
  the same error name with different payload types, and a middleware class that is generic, has no constructor callable without arguments, or extends a generated base without `@ToolMiddleware`.

**Verified on an executor:** a Rust leaf component provides the tools `middleware-probe` (`apply`, and `check` with a camelCase parameter and two declared errors) and
`backend-probe`; a Kotlin component holds a transparent middleware, an adapter with installation parameters and two caller agents using the generated typed client. The host accepted both scopes at deployment (the
adapter's presented tool differs from the one it wraps). A call through the transparent middleware returned `mw(leaf(audit(x)))`, a declared error with a payload and a payloadless declared error both arrived as typed
`ToolInvokeError.Tool` values, and the adapter returned a value computed from the wrapped tool and its parameters.

A declared error whose case has one property carries that property's own type as the payload, not a record of one field, because the host requires an inherited error's payload schema to equal the wrapped tool's and
the Rust SDK defines it that way; a case with two or more properties carries a record.

### Arguments

A command parameter without `@Arg` is a fixed positional. `@Arg` (`cloud.golem.annotations`, placed on the parameter) turns it into another kind of argument; the attribute set follows the Scala SDK's `@arg`:

```kotlin
@ToolDefinition(name = "grep", version = "1.0.0")
interface Grep {
    @ToolCommand(description = "Searches files for a pattern")
    @Constraint(requiresAll = ["pattern"], implies = [Implies(lhs = ["case-sensitive"], rhs = ["context=2"])])
    @ToolResult(formatters = ["human", "json"], default = "json")
    @Example(body = "grep foo src/", title = "basic")
    @ToolAnnotations(readOnly = true, destructive = false)
    fun search(
        pattern: String,
        @Arg(kind = ArgKind.FLAG, short = 'c', negatable = true) caseSensitive: Boolean,
        @Arg(kind = ArgKind.OPTION, short = 'm') maxCount: Int?,
        @Arg(kind = ArgKind.OPTION, default = "2") context: Int,
        @Arg(kind = ArgKind.OPTION, repeatable = Repeat.DELIMITED, delim = ',') include: List<String>,
        @Arg(kind = ArgKind.COUNT_FLAG, maxItems = 3) verbose: UInt,
        @Arg(kind = ArgKind.TAIL) paths: List<String>,
    ): List<String>
}
```

- **Kinds**: `POSITIONAL`; `TAIL` (the last positional, a `List`; `minItems`, `maxItems`, `separator`, `verbatim`); `OPTION` (a nullable type is an optional scalar, a `List` is repeatable, anything else a scalar);
  `FLAG` (`Boolean`; `negatable`); `COUNT_FLAG` (`UInt`, because the wire field is `u32`; `maxItems` bounds the count). Also `short`, `aliases`, `env`, `doc`, `valueName`.
- **Defaults**: `default` is a string parsed by the parameter's type (numbers, `Boolean`, `Char`, `String`, an enum entry name); a nullable or repeatable option and a positional take none.
  The default is metadata for the host's command line; the invocation always carries a value.
- **Constraints**: `@Constraint` (repeatable) names arguments by their kebab-case name or alias: `requiresAll`, `requiresAny`, `allOrNone`, `mutexGroups` (two or more arguments each), `implies`, `forbids`.
  A reference `"name=value"` also requires the value (parsed by the argument's type). `@ToolResult`, `@Example`, `@ToolCommand` (name, description, aliases) and `@ToolAnnotations` complete the command's metadata. `@ToolCommand(name = ...)` names the command instead of the
  method's kebab-cased name, and on a group method it also renames the grafted child root; the handler still follows the Kotlin method.
- **Groups with parameters**: a group method's parameters must be options or flags and become **globals** of every command below it. Calls through the generated client and underlying pass them with the group
  (`underlying.remote(verbose, dir).add(name, url)`), and a handler receives them before the command's own parameters.
- **Input record order** is canonical, not declaration order: the inherited globals (outermost group first, options before flags), the fixed positionals, the tail, the options, the flags. An absent optional
  option is `option<T>`, a flag `bool`, a count flag `u32`, a repeatable option and the tail `list<T>`. The generated code follows it; the host and the Rust SDK use the same order.
- **Refinements**: `min` and `max` (inclusive decimal literals) bound a numeric argument, its item for a list or tail; `regex`, `minLength` and `maxLength` (Unicode scalar values) restrict a `String`.
  A restricted `String` is a `text` in the tool's schema, so its value is a text value; the Kotlin type stays `String`, and the generated code converts. The generator rejects a bound that is not a number of the
  type or does not fit it, `min` above `max`, a default outside the bounds, and a restriction on any other type. The refinements are registered in the tool's schema and sent in the input schema of every
  call through the generated client, and the host rejects a value outside them with `InvalidInput` (`numeric value … is out of range (above maximum 10)`, `text value … does not match required regex`, `… below min-length 2`).
  `ToolArgumentInfo` reports them (`min`, `max`, `minLength`, `maxLength`, `regex`) from reflection. The regex dialect is the host's, so the generator checks a default against it with the JVM's only when the JVM can read the pattern.
- **`unit`**: names the unit of a numeric argument (`@Arg(unit = "ms")`), its item for a list or tail. It reaches the `unit` of the type's `numeric-restrictions` and `ToolArgumentInfo.unit` reports it. It
  describes the value for the schema and for help output rather than constraining it, so the host does not validate against it; the generator rejects it on any type that is not numeric. A generated client
  carries it in the input schema of a call, as it carries the bounds, so the call's schema matches the command's.
- **Paths and urls**: `cloud.golem.schema.GolemPath` and `cloud.golem.schema.Url` are declarable argument types (value classes over a `String`) that reach the schema's `path` and `url` types rather than `string`.
  `@Arg(direction = "input" | "output" | "in-out")`, `@Arg(pathKind = "file" | "directory" | "any")` and `@Arg(mime = [...])` narrow a `GolemPath`'s `path-spec`; `@Arg(schemes = [...])` narrows a `Url`'s
  `url-restrictions`. A `GolemPath` defaults to the widest spec (`in-out`, `any`) and a `Url` to unrestricted. Each facet is rejected on any other declared type, as the Scala SDK rejects a path refinement on a
  non-path: they narrow a declared type and never coerce a `String`. `pathKind` is the parameter's name because `kind` already names the argument kind; its values are the WIT names, so a directory is
  `"directory"` where the Scala SDK writes `"dir"`. A generated client and the CLI exchange these values as `Path("...")` and `Url("...")`.
- **Quantities**: `cloud.golem.schema.Quantity` is a declarable argument type that reaches the schema's `quantity` type. `@QuantityUnit("m", "m", "km")` on the declared type names the base unit and the
  allowed suffixes. A value's unit must be the base unit when there is no suffix, or one of the suffixes when there is — the host's rule, which the generated codecs apply too. There is no `@Arg` facet
  for a quantity: the Scala SDK authors none either, and its `quantity-spec` carries no bounds.
- **Not supported**: `bounds` -- a `(min, max)` pair the Scala SDK takes as one literal, which a Kotlin annotation cannot hold; use `min` and `max`.

### Streams, overlap and cancellation

- `@ToolCommand(stdin = true, stdout = true, stderr = true)` declares a command's byte streams. A client or underlying call of a command with stdin takes a trailing `stdin` parameter (`ByteArray?` on the client,
  `ToolStdin?` on the underlying, so a middleware forwards the invocation's stdin); a command with stdout or stderr returns `Either<ToolInvokeError<E>, WithOutputs<T>>` with the collected bytes. A handler of a stdin
  command receives the invocation's stdin as its last parameter. Outputs are collected whole unless forwarded (below). A parameter named `stdin` is rejected on such a command.
- `XUnderlying.start<Cmd>(…)` starts a wrapped call without waiting and returns a `TypedInvocation` (`TypedInvocationWithOutputs` for a command with outputs) with `await()` and `cancel()`. Several calls can be
  outstanding; `await()` waits for one handle (a second call returns the same outcome), `cancel()` asks the wrapped tool to stop and `await()` then reports `Cancelled`. Handles are released when the handler returns.
  Each handle's outputs are read while that handle is awaited, so await calls that write a lot of output promptly.
- `TypedInvocationWithOutputs.forwardOutputs(callerStdout, callerStderr)` (the writers are `callerStdout` / `callerStderr` inside a handler) writes the wrapped tool's output to the caller as it arrives instead of collecting it.
- Limits: the buffered stream paths (`readAll`, captured stdout and stderr) stop at 16 MiB, and a whole argument or result is bounded by the agent's memory limit; one event loop per awaited handle, not a combined one; crash replay of an outstanding wrapped call.
- **Verified on an executor:** a Rust provider's `arg-probe` tool (positional, tail, optional and defaulted options, a repeatable option, a flag, a count flag) answered a typed call through a Kotlin middleware that started two wrapped calls, awaited the first and cancelled the second; the host's canonical input order matched the generated one, including the globals of a command group (`git-probe` → `remote(verbose, dir)` → `add`); stdin written by a client reached a Rust provider through a forwarding middleware and the provider's stdout came back to the client as it was written.
  A command that declares only stdout must not be called with a stderr attachment: the host rejects it, so the generated client captures only the declared outputs.

## Installation parameters

```kotlin
data class PrefixItem(val name: String, val enabled: Boolean)
data class PrefixConfig(val text: String, val items: List<PrefixItem>)

@ToolMiddleware(name = "prefix", parameters = PrefixConfig::class)
class Prefix : UniversalToolMiddleware {
    override fun invoke(invocation: ToolMiddlewareInvocation, underlying: ToolUnderlying): ToolMiddlewareOutcome {
        val cfg = invocation.parametersAs<PrefixConfig>()
        ...
    }
}
```

The class's schema becomes the middleware's `parameter-schema`; whoever installs the middleware supplies a value of that shape. Verified on a real 1.6.0 executor
(`scripts/tool-middleware/`): a `kt-prefix` middleware installed with a record holding a string and a list of records read `[typed-prefix:first,second]` back through `parametersAs`
and applied it to stdout.
