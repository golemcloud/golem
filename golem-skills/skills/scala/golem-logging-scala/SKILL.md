---
name: golem-logging-scala
description: "Adding logging to a Scala Golem agent. Use when the user asks about logging, log messages, log levels, wasi:logging, golem.wasi.Logging, or printing debug/info/error output from a Scala agent."
---

# Logging from a Scala Agent

The Scala SDK provides `golem.wasi.Logging` — a typed facade over `wasi:logging/logging` with convenience methods for each log level and an optional context parameter.

## Quick Start

Import and use the `Logging` object directly — no initialization needed:

```scala
import golem.wasi.Logging

Logging.info("order created: " + orderId)
Logging.debug("processing item: " + itemId)
Logging.warn("retry attempt " + attempt + " for request " + reqId)
Logging.error("failed to connect: " + err.getMessage)
```

## Log Levels

| Method | WASI Level | Use for |
|--------|-----------|---------|
| `Logging.trace(msg)` | trace | Fine-grained control flow |
| `Logging.debug(msg)` | debug | Debugging information |
| `Logging.info(msg)` | info | Monitoring, normal operations |
| `Logging.warn(msg)` | warn | Hazardous situations |
| `Logging.error(msg)` | error | Serious errors |
| `Logging.critical(msg)` | critical | Fatal errors |

## Context Strings

Every method accepts an optional `context` parameter (defaults to `""`). Use it to scope log entries to a subsystem or request:

```scala
Logging.info("connected to database", context = "db")
Logging.debug(s"query took ${ms}ms", context = "db/query")
Logging.error(s"timeout after ${retries} retries", context = "http-client")
```

For the lowest-level API, call `Logging.log` directly:

```scala
Logging.log(Logging.Level.Critical, "shutdown", "system is shutting down")
```

## Using `println` / `System.err.println`

Standard output and error also work and are captured by Golem:

```scala
println("stdout message")
System.err.println("stderr message")
```

These appear in `golem agent stream` output but lack log levels and context. Prefer `Logging.*` methods for structured, filterable logging.

## Viewing Logs

Stream live agent output:

```shell
golem agent stream '<agent-id>'
```

Or during an invocation:

```shell
golem agent invoke '<agent-id>' '<method>' [args]
```

## Key Points

- **No setup needed** — `golem.wasi.Logging` is available from `golem-scala-core` which is already in the project dependencies
- All methods map directly to the **`wasi:logging/logging`** WASI interface
- Context strings help group and filter log entries by subsystem
- Logs are recorded in the **oplog** and visible via `golem agent stream` and `golem agent invoke`
- Logging is a **side effect** — during replay (crash recovery), log calls from replayed operations are skipped
