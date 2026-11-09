---
name: golem-logging-kotlin
description: "Adding logging to a Kotlin Golem agent. Use when the user asks about logging, log messages, log levels, wasi:logging, or printing debug/info/error output from a Kotlin agent."
---

# Logging in Kotlin Golem Agents

## `Logging`

`cloud.golem.runtime.wasi.Logging` wraps `wasi:logging/logging`:

```kotlin
import cloud.golem.runtime.wasi.Logging

Logging.info("processing order ${order.id}")
Logging.debug("payload: $payload", context = "orders")
Logging.warn("retrying after transient error")
Logging.error("failed to reach payment gateway: ${e.message}")
```

Each level has its own convenience function, taking the message and an optional `context` string (default `""`):

| Function | Level |
|---|---|
| `Logging.trace(message, context)` | `TRACE` |
| `Logging.debug(message, context)` | `DEBUG` |
| `Logging.info(message, context)` | `INFO` |
| `Logging.warn(message, context)` | `WARN` |
| `Logging.error(message, context)` | `ERROR` |
| `Logging.critical(message, context)` | `CRITICAL` |

Or call `Logging.log(level: LogLevel, context: String, message: String)` directly with a `LogLevel` enum value.

## Plain `println`

`println(...)` also works and is captured the same way as any other guest stdout; use `Logging` when you want an explicit level or context, since `println` always logs at a fixed level with no context.

## Where logs go

- `golem agent invoke` and `golem agent new` stream an agent's logs live to the terminal by default (`--no-stream` disables this — see `golem-invoke-agent-kotlin`).
- `golem agent log` / `golem-view-agent-logs` reads an agent's recorded log history after the fact.
- If the `golem-otlp-exporter` plugin is enabled with `logs` in its `signals`, log output is also forwarded to the configured OTLP collector — see `golem-enable-otlp-kotlin`.

## Related skills

- `golem-view-agent-logs`
- `golem-enable-otlp-kotlin`
