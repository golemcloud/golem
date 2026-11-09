---
name: golem-tools-middleware-kotlin
description: "Writing tool middleware in Kotlin. Use when the user asks to wrap, transform, log or reject tool invocations, or configure a middleware with installation parameters."
---

# Tool Middleware (Kotlin)

A middleware wraps tool invocations: it sees the tool name, command path and typed input, may call the wrapped tool, transform its result and output, or reject the call.

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
        ToolMiddlewareOutcome.Err(ToolError.ConstraintViolation("denied: ${invocation.toolName}"))
}
```

- Requirements: a public no-argument constructor, implement `UniversalToolMiddleware`, annotate with `@ToolMiddleware(name, version, aliases, description, parameters)`. A fresh instance is created per top-level invocation: keep no state.
- `aliases` are alternative names the middleware answers to: `@ToolMiddleware(name = "audit", aliases = ["trace", "log"])`.
- Declare the middleware in the manifest, or the deploy refuses the component with `has no matching top-level declaration`:

```yaml
tools:
  middleware:
    audit:
      component: example:mw
```
- `ToolUnderlying.invoke(path, input, stdin)` calls the next layer; outputs are **buffered** byte arrays. Forward `invocation.stdin` to pass stdin on (once).
- Results: `ToolMiddlewareOutcome.Ok(result, stdout, stderr)`, `.Err(ToolError)`, `.passThrough(outcome)`.
- Installation parameters: `@ToolMiddleware(name = "prefix", parameters = PrefixConfig::class)` makes the data class the parameter schema; read it with `invocation.parametersAs<PrefixConfig>()`.
- An exception thrown by the middleware becomes `invalid-input` (a `ToolException` becomes `constraint-violation`).
- `invocation` exposes `middlewareName`, `toolName`, `parameters`, `commandPath`, `input`, `stdin` (`ToolInput?`) and `principal`. `UnderlyingOutcome.Err` carries an `UnderlyingError` (`ToolFailure`, `ProtocolError`, `Denied`, `InternalError`, `Cancelled`, `ResourceExhausted`).
- Only universal (wrap-any-tool) middleware is supported; typed per-tool middleware is not.

## Related skills

- `golem-define-tool-kotlin`
- `golem-call-tool-kotlin`
