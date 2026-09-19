# golem-rust

A library that help writing [Golem](https://golem.cloud) programs by providing higher level Rust
wrappers for Golem's runtime APIs, including functions for defining and performing operations
transactionally.

## Tool middleware

`#[tool_middleware]` and `#[universal_tool_middleware]` accept `parameters = P` for statically typed installation parameters. `P` must implement the SDK schema conversion traits. For monomorphic middleware, the declared `constructor` has signature `fn(P) -> Self`; universal middleware receives `P` as its first function argument. Without `parameters`, constructors remain zero-argument and universal functions have no parameter value.

Generated typed underlying proxies provide awaited command methods and `start_<command>(...)`. Each started `TypedUnderlyingInvocation` has independent `get()`, public optional `stdout`, and `cancel()`, so calls may overlap and results and stdout may be observed in either order. Universal `UnderlyingTool` provides the corresponding `start_with(...)`; `invoke(...)` remains the convenient awaited form.

Sequential and concurrent `get()` calls on the same observer share one host observation and return the cached terminal result, including errors. This does not duplicate or rewind stdout. Underlying `Cancelled` and `ResourceExhausted` errors remain distinguishable to middleware code; they become `ConstraintViolation` only when forwarded as the middleware's own wire result.

For structural-subtype and nominal compatibility, every inner tool error must be declared by the expected tool with a compatible payload. Expected-only errors are allowed; inner-only errors are rejected. Strict equality requires matching error vocabularies.

Returning from the handler revokes new admissions but does not implicitly cancel admitted calls. Dropping a result observer releases observation rather than cancelling the invocation; call `cancel()` explicitly when intended. The SDK disposes abandoned observers and streams.

Universal middleware also receives the invocation's optional `OutputStream`. For pass-through, use `invoke_forwarding_stdout(command_path, input, stdin, stdout)`. Typed started calls with declared stdout use `get_forwarding_stdout(stdout)`. These helpers copy readable underlying stdout into the host writer concurrently with the structured result, finish it after clean EOF, and fail it on forwarding errors.
