---
name: golem-logging-ts
description: "Adding logging to a TypeScript Golem agent. Use when the user asks about logging, console.log, log levels, printing debug/info/error output, or wasi:logging from a TypeScript agent."
---

# Logging from a TypeScript Agent

TypeScript agents log using the standard `console` API. Golem captures stdout and stderr and streams them to `golem agent stream` and `golem agent invoke` output.

## Quick Start

Use the built-in `console` methods — no imports or setup needed:

```typescript
console.log("order created:", orderId);
console.info("processing item", itemId);
console.warn("retry attempt", attempt, "for request", reqId);
console.error("failed to connect to database:", err);
console.debug("detailed flow: entered processItem");
```

## Log Levels

| Method | Behavior | Use for |
|--------|----------|---------|
| `console.trace(...)` | Prints with stack trace to stderr | Fine-grained debugging with call stack |
| `console.debug(...)` | Writes to stdout | Debugging information |
| `console.log(...)` | Writes to stdout | General output |
| `console.info(...)` | Writes to stdout | Monitoring, normal operations |
| `console.warn(...)` | Writes to stderr | Hazardous situations, degraded behavior |
| `console.error(...)` | Writes to stderr | Serious errors |

## Structured Logging

For structured output, log JSON objects:

```typescript
console.log(JSON.stringify({
  level: "info",
  event: "order_created",
  orderId,
  timestamp: new Date().toISOString(),
}));
```

Or use template literals for readable messages with context:

```typescript
console.info(`[${agentName}] counter incremented to ${this.count}`);
```

## Viewing Logs

Stream live agent output (stdout and stderr):

```shell
golem agent stream '<agent-id>'
```

Or during an invocation:

```shell
golem agent invoke '<agent-id>' '<method>' [args]
# Logs stream automatically; use --no-stream to suppress
```

## Key Points

- **No setup needed** — `console` is available globally in the QuickJS runtime
- Golem captures both **stdout** (`console.log`, `console.info`, `console.debug`) and **stderr** (`console.warn`, `console.error`) streams
- Logs are recorded in the **oplog** and visible via `golem agent stream` and `golem agent invoke`
- Logging is a **side effect** — during replay (crash recovery), log output from replayed operations is skipped; only new invocations produce log output
- The TypeScript SDK does not currently expose a direct `wasi:logging/logging` binding — use `console` methods for all logging needs
