---
name: golem-logging-go
description: "Structured logging from a Go Golem agent via log/slog. Use when the user asks about logging, log levels, structured/key-value logs, or printing debug/info/error output from a Go agent."
---

# Logging from a Go Agent

## Overview

Golem routes Go logging through the host's structured logging channel (`wasi:logging`), so each record carries a typed **level** and a **context** (category) string and shows up in worker logs and the oplog with the right severity. Unlike writing to stdout/stderr — which the host records as raw bytes with no level — this gives you filterable, leveled logs.

The runtime installs the SDK's handler as the **default `log/slog` handler on agent startup**, so ordinary standard-library structured logging just works with **no setup code**. You only reach for the SDK's `log` package (`github.com/golemcloud/golem/sdks/go/golem/log`) when you want to customize the level or category, or build a handler yourself.

## Steps

1. **Log with the standard library** — call `slog.Info`, `slog.Warn`, etc. Nothing to import beyond `log/slog`.
2. **(Optional) Customize** the minimum level or base category with `log.SetDefault(...)` from the SDK's `log` package.
3. **View logs** with `golem agent stream` or during `golem agent invoke`.

## Quick Start (no setup)

The runtime already installed the host-logging handler as slog's default, so just use `log/slog`:

```go
import "log/slog"

slog.Info("order created", "orderID", orderID, "total", total)   // -> level=info
slog.Warn("retrying", "attempt", attempt, "reqID", reqID)        // -> level=warn
slog.Error("db connect failed", "err", err)                      // -> level=error
slog.Debug("processing item", "id", item.ID)                     // -> level=debug
```

Structured context is passed as trailing key/value pairs (slog attributes) and is rendered into the log record. `slog.With(...)` returns a logger that prepends attributes to every record:

```go
logger := slog.With("area", "billing")
logger.Error("declined", "code", code)
```

Because `slog.SetDefault` also bridges Go's standard `log` package, plain `log.Print` / `log.Printf` output flows through the same channel too (as `info`-level records).

## Log Levels

Records map onto the `wasi:logging` severities. slog's built-in levels cover debug/info/warn/error; the SDK also defines trace and critical (`sdks/go/golem/log/log.go:47`):

| slog call | Level | Use for |
|-----------|-------|---------|
| `slog.Log(ctx, level-4, ...)` (below Debug) | `trace` | Fine-grained control flow |
| `slog.Debug` | `debug` | Debugging information |
| `slog.Info` | `info` | Normal operations, monitoring |
| `slog.Warn` | `warn` | Degraded behavior, hazards |
| `slog.Error` | `error` | Serious errors |
| `slog.Log` a full step above Error | `critical` | Fatal conditions |

The SDK exposes the raw severities as `log.Trace`, `log.Debug`, `log.Info`, `log.Warn`, `log.Error`, `log.Critical` (type `log.Level`) for the low-level `log.Log` helper.

## Customizing the default logger

Import the SDK's `log` package (alias it to avoid clashing with the standard `log`) and call `log.SetDefault` to change the minimum level or the base category. `Options` has just `Level` (an `slog.Leveler`) and `Context` (the base category string) — see `sdks/go/golem/log/log.go:103`.

```go
import (
	"log/slog"

	golemlog "github.com/golemcloud/golem/sdks/go/golem/log"
)

func init() {
	// Emit debug-and-up, tagging every record with the "billing" category.
	golemlog.SetDefault(&golemlog.Options{
		Level:   slog.LevelDebug,
		Context: "billing",
	})
}
```

## Building a handler yourself

`log.NewHandler(opts)` returns an `*slog.Handler` you can pass to `slog.New` (for a scoped logger, or to compose):

```go
handler := golemlog.NewHandler(&golemlog.Options{Context: "worker"})
logger := slog.New(handler)
logger.Info("started")
```

slog groups map onto the category: `logger.WithGroup("retry")` extends the context to read like `worker/retry` rather than key-prefixing attributes.

## Lower-level helpers

- `log.Log(level, context, message)` — emit one raw record directly (`sdks/go/golem/log/log.go:58`).
- `log.Writer(level, context)` — an `io.Writer` where each write becomes one record at that level/category, handy for wiring a sink that expects an `io.Writer`.

```go
golemlog.Log(golemlog.Warn, "startup", "cache warm skipped")
```

## Viewing Logs

Stream live agent output (stdout, stderr, and the log channel):

```shell
golem agent stream '<agent-id>'
```

Logs also stream automatically during an invocation:

```shell
golem agent invoke '<agent-id>' '<method>' [args]
# use --no-stream to suppress
```

## Key Constraints

- **No initialization needed** — the runtime installs the host-logging handler as slog's default on startup; just use `log/slog`.
- Prefer `slog.*` over `fmt.Println`/stdout: stdout/stderr are captured as raw bytes with no level or category.
- Logging is a **side effect**: during replay (crash recovery) log calls from replayed operations are skipped — only new invocations produce log output.
- The SDK's package is named `log`, which collides with the standard library `log`; import it under an alias (e.g. `golemlog`).
- `int`/`uint`-width and other value formatting follow slog's attribute rendering (values are resolved and stringified into the message).

### Related Skills

| Skill | When to Load |
|-------|--------------|
| `golem-enable-otlp-go` | Forward logs (and traces/metrics) to an OTLP collector |
| `golem-add-agent-go` | Define the agent whose handlers emit these logs |
