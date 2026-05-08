---
name: golem-logging-moonbit
description: "Adding logging to a MoonBit Golem agent. Use when the user asks about logging, log messages, log levels, wasi:logging, @logging, or printing debug/info/error output from a MoonBit agent."
---

# Logging from a MoonBit Agent

The MoonBit SDK provides a high-level `@logging` module (from `golemcloud/golem_sdk/logging`) built on top of `wasi:logging/logging`. It supports named loggers, log levels, and global level filtering.

## Quick Start

The `@logging` package is already imported in the template `moon.pkg`. Use the module-level convenience functions:

```moonbit
@logging.info("order created: " + order_id)
@logging.debug("processing item: " + item_id)
@logging.warn("retry attempt " + attempt.to_string())
@logging.error("failed to connect: " + err)
```

## Using a Named Logger

Create a logger with a context name for structured, scoped output. The context string is passed as the WASI `context` parameter:

```moonbit
let logger : @logging.Logger = @logging.with_name("order-service")

fn process_order(self : Self, order_id : String) -> Unit {
  logger.info("processing order: " + order_id)
  logger.debug("validating items")
}
```

Loggers are immutable values — `with_name` appends a `/`-separated segment to the context:

```moonbit
let db_logger = logger.with_name("database")
// context is "order-service/database"
db_logger.info("connected to database")
```

## Log Levels

| Function | WASI Level | Use for |
|----------|-----------|---------|
| `@logging.trace(...)` / `logger.trace(...)` | trace | Fine-grained control flow |
| `@logging.debug(...)` / `logger.debug(...)` | debug | Debugging information |
| `@logging.info(...)` / `logger.info(...)` | info | Monitoring, normal operations |
| `@logging.warn(...)` / `logger.warn(...)` | warn | Hazardous situations |
| `@logging.error(...)` / `logger.error(...)` | error | Serious errors |
| `@logging.critical(...)` / `logger.critical(...)` | critical | Fatal errors |

## Level Filtering

Set a global minimum log level to suppress noisy output:

```moonbit
@logging.set_min_level(INFO)  // suppresses trace and debug globally
```

Per-logger overrides are also supported:

```moonbit
let verbose_logger = logger.with_min_level(TRACE)
```

If a logger has no per-instance level, it uses the global minimum (default: `TRACE`).

## Setting a Global Context

Set a default context string for all module-level logging functions:

```moonbit
@logging.set_context("my-agent")
@logging.info("started")  // context = "my-agent"
```

## Using `println`

MoonBit's `println` also works and is captured as stdout:

```moonbit
println("stdout message")
```

This appears in `golem agent stream` output but lacks log levels and context. Prefer `@logging` for structured, filterable logging.

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

- **No initialization needed** — the `@logging` package is ready to use out of the box
- **`@logging` is pre-imported** in the template `moon.pkg` as `"golemcloud/golem_sdk/logging" @logging`
- Named loggers via `@logging.with_name(...)` provide scoped context strings
- Logs are recorded in the **oplog** and visible via `golem agent stream` and `golem agent invoke`
- Logging is a **side effect** — during replay (crash recovery), log calls from replayed operations are skipped
