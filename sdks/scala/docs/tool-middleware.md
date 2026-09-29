# Tool middleware

The Scala SDK supports transparent and adapter middleware for a specific tool definition, plus universal middleware that can wrap any tool. Middleware invocations are asynchronous and receive an invocation-scoped underlying handle supplied by the runtime.

The complete compiling examples live in [`ToolMiddlewareCompileFixture.scala`](../test-agents/src/main/scala/example/integrationtests/ToolMiddlewareCompileFixture.scala). Equivalent transparent, adapter, and universal definitions are also compiled by the Mill fixture under [`mill/test-fixture`](../mill/test-fixture/).

## Monomorphic middleware

For each `@toolDefinition` trait, the sbt and Mill plugins generate these middleware-facing APIs in addition to the ordinary client:

- `<Tool>Underlying`: asynchronous typed calls through the runtime-supplied wrapped tool;
- `<Tool>Middleware`: the transparent middleware surface;
- `<Tool>Middleware.Adapter[U]`: the same presented surface with a different expected underlying type `U`.

Every generated middleware method takes its underlying as the first parameter and returns `Future[Either[ToolInvokeError[E], A]]`. Global arguments, command arguments, and `Principal` follow the generated projection for that command. Declared stdin becomes a `ToolMiddlewareInputHandle`; declared stdout is carried by the successful result as a `ToolMiddlewareOutputHandle`.

Installation parameters are statically typed. Extend `<Tool>Middleware.WithParameters[P]` (or `UniversalToolMiddleware.WithParameters[P]`) and use `UniversalToolMiddlewareInvocation[P]`; monomorphic generated handlers receive `parameters: P` immediately after the underlying. `P` must have a `zio.blocks.schema.Schema`. The no-configuration universal form uses `UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters]`. See the compile fixture linked above for both forms.

### Transparent middleware

A transparent middleware presents and expects the same tool. Extend the generated `<Tool>Middleware` trait:

```scala
import golem.Principal
import golem.runtime.annotations.toolMiddleware
import golem.tool.ToolInvokeError

import scala.concurrent.Future

@toolMiddleware(name = "middleware-fixture-transparent")
final class MiddlewareFixtureTransparent extends MiddlewareFixtureToolMiddleware {
  def call(
    underlying: MiddlewareFixtureToolUnderlying,
    config: String,
    value: String,
    principal: Principal
  ): Future[Either[ToolInvokeError[MiddlewareFixtureError], String]] =
    underlying.call(config, value).toMiddlewareResult
}
```

Middleware classes must be concrete, non-generic, accessible to generated registration code, and constructible with no arguments. The runtime constructs a fresh middleware instance for every top-level invocation. The underlying is passed to each method; never put it in constructor or object state.

### Adapter middleware

An adapter presents one tool while expecting another. Extend the presented tool's generated `Adapter` trait with the expected tool's generated underlying:

```scala
@toolMiddleware(name = "middleware-fixture-adapter")
final class MiddlewareFixtureAdapter
    extends MiddlewareFixtureToolMiddleware.Adapter[MiddlewareFixtureBackendUnderlying] {
  def call(
    underlying: MiddlewareFixtureBackendUnderlying,
    config: String,
    value: String,
    principal: Principal
  ): Future[Either[ToolInvokeError[MiddlewareFixtureError], String]] =
    underlying
      .execute(value)
      .toMiddlewareResult
      .map {
        case Right(length) => Right(s"$config:$length")
        case Left(error)   =>
          Left(error.mapTool { case MiddlewareFixtureBackendError.Failed(message) =>
            MiddlewareFixtureError.Rejected(message)
          })
      }(scala.concurrent.ExecutionContext.parasitic)
}
```

The adapter owns all semantic conversion between the expected and presented tools: command inputs, successful values, stdout shape, and custom errors. `ToolInvokeError.mapTool` converts only the custom `Tool(E)` payload and preserves protocol errors unchanged.

## Universal middleware

A universal middleware receives raw metadata and invocation values and can wrap any tool:

```scala
import golem.runtime.annotations.universalToolMiddleware
import golem.schema.TypedSchemaValue
import golem.tool.{
  ToolInvokeError,
  ToolMiddleware,
  ToolMiddlewareResult,
  UniversalToolMiddleware,
  UniversalToolMiddlewareInvocation,
  UniversalToolUnderlying
}

import scala.concurrent.Future

@universalToolMiddleware(name = "middleware-fixture-universal")
final class MiddlewareFixtureUniversal extends UniversalToolMiddleware {
  def invoke(
    invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
    underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
}
```

`UniversalToolMiddlewareInvocation` contains the tool name, full tool metadata, command path, typed schema input, optional stdin, and principal. Universal custom errors remain raw `TypedSchemaValue` payloads.

## Exact invocation errors

Middleware uses `ToolInvokeError[+E]`, not the ambient client's `ToolError[E]`:

```scala
sealed trait ToolInvokeError[+E]

ToolInvokeError.InvalidToolName(name)
ToolInvokeError.InvalidCommandPath(path)
ToolInvokeError.InvalidInput(message)
ToolInvokeError.ConstraintViolation(message)
ToolInvokeError.InvalidResult(message)
ToolInvokeError.Tool(error: E)
```

Return these errors in `Left` when rejecting an invocation deliberately. A monomorphic middleware method uses the presented tool's declared error type, while calls on an adapter's underlying use the expected tool's declared error type. Universal middleware uses `TypedSchemaValue`. A failed `Future` is not a declared tool error: it remains an unhandled component failure/trap.

## Underlying lifetime and call ordering

The supplied underlying is affine and valid only during its middleware invocation:

- Convenience calls return `ToolUnderlyingInvocation`; call `.toMiddlewareResult` for the former await-the-whole-call behavior.
- Calls may overlap. Each invocation has an independent admission, result, stdout, `cancel`, and `drop` handle, so middleware can fan out and observe completions in any order.
- Sequential and concurrent `get()` calls share one lazy host observation and return the cached terminal result, including errors. This does not duplicate or rewind stdout.
- Do not store, return, capture for later, or otherwise let the underlying escape. It is revoked when the middleware handler returns.
- Revocation at handler return prevents new admissions but does not implicitly cancel admitted calls. Cleanup requests disposal of their observers; when observation is pending, disposal waits for it to settle.
- Dropping an observer releases observation; it is not cancellation. Invoke its `cancel` callback only when cancellation is intended.

Underlying `ToolInvokeError.Cancelled` and `ToolInvokeError.ResourceExhausted` remain distinguishable to middleware code. They become `ConstraintViolation` only when forwarded as the middleware's own wire result. Dropping an unobserved admission disposes it immediately without starting a host `get`.

For structural-subtype and nominal compatibility, every inner tool error must be declared by the expected tool with a compatible payload. Expected-only errors are allowed; inner-only errors are rejected. Strict equality requires matching error vocabularies.

Post-invocation admission fails with `ToolUnderlyingMisuseException`; this is SDK misuse, not a `ToolInvokeError` returned by the wrapped tool.

## Stream transfer and cleanup

`ToolMiddlewareInputHandle` and `ToolMiddlewareOutputHandle` are transfer-only capabilities. They intentionally have no public `read`, `cancel`, `write`, `finish`, or `fail` methods. Middleware can forward and select them, but byte-level stream consumption and production belong to ordinary tool implementations and callers.

The handles follow the same invocation ownership:

- Passing the invocation's stdin to an underlying call transfers it exactly once. If it is never forwarded, the SDK closes it when the middleware settles.
- Forwarding the same stream twice is SDK misuse.
- Stdout returned from underlying calls is tracked. Intermediate, abandoned, malformed, or error-path stdout is closed best-effort.
- Only the stdout selected in the middleware's final successful result is transferred to the caller; it remains open for the caller.
- Cleanup is identity-based and idempotent, including when the same stdout handle appears more than once.

The guest ABI supplies a stdout writer for commands that declare stdout. The SDK copies the selected final stdout into that writer while the structured result is pending, calls `finish` after clean EOF, and calls `fail` if forwarding fails. Middleware receives only the transfer-oriented handles above; it must not finish or fail the host writer itself.

## Component template

Use the `scala` component template for ordinary agents and tools, standalone middleware, and
components combining them. Its `agent_guest.wasm` base artifact exports all three discovery and
invocation interfaces and imports the ambient tool host. Categories the component does not define
return empty discovery lists. The invocation-scoped `underlying` capability remains the way to
advance the pinned middleware chain; ambient tool calls do not bypass runtime permission checks.

The artifact is embedded byte-for-byte in both sbt and Mill plugins. `golemPrepare` refreshes the
`.generated/agent_guest.wasm` file by content hash.

## Future client design

[GOL-484](https://linear.app/golem-cloud/issue/GOL-484/redesign-scala-typed-tool-clients-around-injectable-transports-and) tracks a possible redesign of typed Scala tool clients around injectable transports and failure algebras. That could simplify how ordinary and underlying projections share implementation, but it is not required to author or run middleware with the API described here.
