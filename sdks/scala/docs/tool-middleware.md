# Tool middleware

The Scala SDK supports transparent and adapter middleware for a specific tool definition, plus universal middleware that can wrap any tool. Middleware invocations are asynchronous and receive an invocation-scoped underlying handle supplied by the runtime.

The complete compiling examples live in [`ToolMiddlewareCompileFixture.scala`](../test-agents/src/main/scala/example/integrationtests/ToolMiddlewareCompileFixture.scala). Equivalent transparent, adapter, and universal definitions are also compiled by the Mill fixture under [`mill/test-fixture`](../mill/test-fixture/).

## Monomorphic middleware

For each `@toolDefinition` trait, the sbt and Mill plugins generate these middleware-facing APIs in addition to the ordinary client:

- `<Tool>Underlying`: asynchronous typed calls through the runtime-supplied wrapped tool;
- `<Tool>Middleware`: the transparent middleware surface;
- `<Tool>Middleware.Adapter[U]`: the same presented surface with a different expected underlying type `U`.

Every generated middleware method takes its underlying as the first parameter and returns `Future[Either[ToolInvokeError[E], A]]`. Global arguments, command arguments, `Principal`, stdin, and stdout follow the generated projection for that command.

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
    underlying.call(config, value)
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
  ToolInvokeResult,
  UniversalToolMiddleware,
  UniversalToolMiddlewareInvocation,
  UniversalToolUnderlying
}

import scala.concurrent.Future

@universalToolMiddleware(name = "middleware-fixture-universal")
final class MiddlewareFixtureUniversal extends UniversalToolMiddleware {
  def invoke(
    invocation: UniversalToolMiddlewareInvocation,
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] =
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

- Sequential calls are allowed. A middleware may await one call and then invoke the underlying again, for example to retry.
- Calls must not overlap. Start the next call only after the previous call's `Future` settles.
- Do not store, return, capture for later, or otherwise let the underlying escape. It is revoked when the middleware invocation settles.
- Overlapping or post-invocation calls fail with `ToolUnderlyingMisuseException`; this is SDK misuse, not a `ToolInvokeError` returned by the wrapped tool.

The SDK waits for an active underlying call before revoking the handle and cleaning up the invocation.

## Stream transfer and cleanup

Stdin and stdout handles follow the same invocation ownership:

- Passing the invocation's stdin to an underlying call transfers it exactly once. If it is never forwarded, the SDK closes it when the middleware settles.
- Forwarding the same stream twice is SDK misuse.
- Stdout returned from underlying calls is tracked. Intermediate, abandoned, malformed, or error-path stdout is closed best-effort.
- Only the stdout selected in the middleware's final successful result is transferred to the caller; it remains open for the caller.
- Cleanup is identity-based and idempotent, including when the same stdout handle appears more than once.

The current Scala stream types are opaque handles. Middleware can safely forward and select them, but byte-level stream authoring is outside this API.

## Choosing a component role

Use the component template matching the exports and host access the component needs:

| Template | Base artifact | Use when | Ambient tool host |
|---|---|---|---|
| `scala` | `agent_guest.wasm` | The component contains ordinary agents or tools, but no middleware export | Available |
| `scala-tool-middleware` | `tool_middleware_guest.wasm` | The component contains only tool middleware | Not imported |
| `scala-agent-tool-middleware` | `agent_tool_middleware_guest.wasm` | The component combines ordinary agents/tools and middleware | Available |

A pure middleware component cannot invoke ambient tools through an ordinary generated `<Tool>Client`; its final WASM intentionally has no `golem:tool/host@0.1.0` import. It can call only the invocation-scoped underlying supplied to the middleware method. Use the combined role only when the same component genuinely needs both export surfaces and ambient tool access.

The three artifacts are generated from the same WIT/build matrix and embedded byte-for-byte in both sbt and Mill plugins. `golemPrepare` refreshes all three `.generated/*.wasm` files by content hash, while `golem-cli` selects the one named by the component template.

## Future client design

[GOL-484](https://linear.app/golem-cloud/issue/GOL-484/redesign-scala-typed-tool-clients-around-injectable-transports-and) tracks a possible redesign of typed Scala tool clients around injectable transports and failure algebras. That could simplify how ordinary and underlying projections share implementation, but it is not required to author or run middleware with the API described here.
