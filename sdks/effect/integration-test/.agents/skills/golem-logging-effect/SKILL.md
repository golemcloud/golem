---
name: golem-logging-effect
description: "Adding structured logging and tracing spans to an Effect-based Golem agent. Use when adding log messages, log levels, Effect.log*, annotations, error causes, spans, or streamed agent output with @golemcloud/effect-golem."
---

# Logging and Tracing from an Effect Agent

`@golemcloud/effect-golem` automatically installs a host-backed Effect logger and tracer for
agent initialization, method handlers, and user-defined snapshot handlers. Use normal Effect v4
logging and span operators; do not install logging or tracing layers in the agent.

Logs accepted by Effect's level filter are forwarded to `wasi:logging/logging`. The SDK includes
log annotations, log spans, and the active host trace and span IDs in a logfmt-style line.

## Complete Agent Example

Logging calls are Effects, so compose or `yield*` them inside the method handler. Effect's default
minimum log level is `"Info"`; lower it around a handler when debug or trace output is required.

```typescript
import { Effect, References, Schema } from "effect";
import { defineAgent, method } from "@golemcloud/effect-golem";

export const LogDemoAgent = defineAgent({
  name: "LogDemoAgent",
  mode: "durable",
  constructorParams: {
    instanceName: Schema.String,
  },
  methods: {
    doWork: method({
      params: { taskName: Schema.String },
      success: Schema.String,
    }),
  },
}).implement(({ instanceName }) =>
  Effect.succeed({
    doWork: ({ taskName }) =>
      Effect.gen(function* () {
        yield* Effect.logInfo(`starting task: ${taskName}`);
        yield* Effect.logDebug(`processing task: ${taskName}`);
        yield* Effect.logWarning(`task is slow: ${taskName}`);
        return "done";
      }).pipe(
        Effect.annotateLogs({ instanceName, taskName }),
        Effect.withSpan("LogDemoAgent.doWork", {
          attributes: { instanceName, taskName },
        }),
        Effect.provideService(References.MinimumLogLevel, "Debug"),
      ),
  }),
);
```

Register the implementation from the component entry point:

```typescript
// src/main.ts
import "./log-demo-agent.js";
```

Use the emitted `.js` suffix for local imports because generated Effect projects use ESM with
NodeNext module resolution.

## Log Levels

| Effect function          | Golem/WASI level | Use for                              |
| ------------------------ | ---------------- | ------------------------------------ |
| `Effect.logTrace(...)`   | trace            | Fine-grained control flow and values |
| `Effect.logDebug(...)`   | debug            | Development and diagnostic details   |
| `Effect.logInfo(...)`    | info             | Significant normal operations        |
| `Effect.logWarning(...)` | warn             | Recoverable or degraded situations   |
| `Effect.logError(...)`   | error            | Serious operation failures           |
| `Effect.logFatal(...)`   | critical         | Fatal failures                       |

The warning function is named `logWarning`, not `logWarn`.

## Structured Context and Causes

Prefer stable messages with dynamic values in `Effect.annotateLogs` when the exact rendered
message is not part of an output contract:

```typescript
const createOrder = Effect.gen(function* () {
  yield* Effect.logInfo("order created").pipe(
    Effect.annotateLogs({ orderId, customerId }),
  );
});
```

Applying `Effect.annotateLogs({...})` to a whole handler Effect adds the fields to every log in
that scope. Annotation values are safely rendered into the host log line.

Observe an Effect failure without changing it by tapping its full cause:

```typescript
const observed = operation.pipe(
  Effect.tapCause((cause) => Effect.logError("order processing failed", cause)),
);
```

The Golem logger renders the Effect `Cause` in a `cause` field. In Effect v4 the operator is
`Effect.tapCause`, not the Effect v3 name `tapErrorCause`.

## Debug and Trace Filtering

Effect's default minimum level is `"Info"`, so `logDebug` and `logTrace` are normally filtered
before reaching Golem. Override the threshold only around the Effect that needs verbose output:

```typescript
const verboseHandler = handler.pipe(
  Effect.provideService(References.MinimumLogLevel, "Debug"),
);
```

Use `"Trace"` to include trace logs or `"All"` to accept every level. This fiber-context-local
override also applies to child fibers started by the wrapped Effect; it does not lower the level
for unrelated handlers.

## Log Spans and Tracing Spans

These two span operators serve different purposes:

- `Effect.withLogSpan("lookup")` adds a named duration to log lines in its scope.
- `Effect.withSpan("OrderAgent.process", { attributes: {...} })` creates a real child tracing
  span under the Golem invocation span. Logs inside it include the active trace and span IDs.

Add tracing attributes discovered during execution from inside an active tracing span:

```typescript
const processOrder = Effect.gen(function* () {
  const queue = "priority";
  yield* Effect.annotateCurrentSpan({ queue, retryable: true });
  yield* Effect.logInfo("order accepted").pipe(Effect.annotateLogs({ queue }));
}).pipe(Effect.withSpan("OrderAgent.process"));
```

`Effect.withSpan` finishes the host span on success, failure, or interruption. Failed Effects mark
the span as an error. Do not manually finish it. Host span attributes are string-valued, so the SDK
safely renders non-string Effect attribute values before forwarding them.

## Viewing Logs

Invocations stream logs by default:

```shell
golem agent invoke 'LogDemoAgent("demo")' doWork '"my-task"'
# Add --no-stream to suppress live log output.
```

Stream an agent's output independently:

```shell
golem agent stream 'LogDemoAgent("demo")'
```

Golem records logs in the oplog. Logging is a durable side effect: replay skips log calls from
already-recorded operations, so recovery does not duplicate their output.

## Key Constraints

- Import Effect APIs from `effect` and agent APIs from `@golemcloud/effect-golem`.
- Prefer `Effect.log*` over `console.*` and the lower-level `Logging.log(...)`; Effect logging keeps
  annotations, log spans, causes, and trace correlation.
- Do not invent methods such as `Logging.info(...)` or `Effect.logWarn(...)`.
- Do not provide `Logging.layer` or `Tracing.layer`; the agent dispatcher already installs both.
- Keep the generated `effect` and `@golemcloud/effect-golem` versions aligned.
- Do not edit generated files under `golem-temp/`.
