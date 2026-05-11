---
name: golem-local-dev-server
description: "Starting, configuring, and debugging the local Golem development server with `golem server`. Use when asked to start, stop, clean, or configure the local Golem server, or when you need to enable debug logs, find a useful tracing target, or diagnose runtime behavior of a deployed agent (e.g. status-code retry not firing, semantic trap retry decisions, durability events)."
---

# Local Golem Development Server (`golem server`)

The `golem server` command runs a self-contained Golem server on the local machine for development and testing. It bundles all Golem services (worker executor, component compilation, shard manager, registry, and router) into a single process.

**Note:** Only the `golem` binary supports this command. `golem-cli` does not include `golem server`.

## Subcommands

### `golem server run`

Starts the local Golem server.

```shell
golem server run
```

#### Parameters

| Parameter | Description | Default |
|-----------|-------------|---------|
| `--router-addr <ADDR>` | Address to serve the main API on | `0.0.0.0` |
| `--router-port <PORT>` | Port to serve the main API on | `9881` |
| `--custom-request-port <PORT>` | Port to serve custom HTTP requests on (HTTP API endpoints) | `9006` |
| `--mcp-port <PORT>` | Port to serve the MCP server on | `9007` |
| `--ports-file <PATH>` | Write discovered startup ports to this JSON file | _(none)_ |
| `--data-dir <PATH>` | Directory to store data in | Platform-specific (see below) |
| `--clean` | Clean the data directory before starting | `false` |
| `--agent-filesystem-root <PATH>` | Use deterministic agent filesystem directories rooted at the given path instead of random temp directories. Layout: `<root>/<environment_id>/<component_id>/<agent_name>/` | _(none)_ |

#### Examples

Start with defaults:

```shell
golem server run
```

Start on a custom port:

```shell
golem server run --router-port 8080
```

Start fresh, deleting all previous state:

```shell
golem server run --clean
```

Start with a custom data directory and deterministic agent filesystems:

```shell
golem server run --data-dir ./my-data --agent-filesystem-root ./agent-fs
```

Write port information to a file (useful for scripting and CI):

```shell
golem server run --ports-file ./ports.json
```

#### Default Data Directory

The default data directory is platform-specific:

| Platform | Default Path |
|----------|-------------|
| **macOS** | `~/Library/Application Support/golem` |
| **Linux** | `~/.local/share/golem` |
| **Windows** | `C:\Users\<USER>\AppData\Local\golem` |

#### Ports File Format

When `--ports-file` is specified, the server writes a JSON file with the actual ports it bound to. This is useful when using port `0` (OS-assigned) or for scripting and CI automation. The file is written atomically (via a `.tmp` rename) once all services are ready.

```json
{
  "routerPort": 9881,
  "customRequestPort": 9006,
  "mcpPort": 9007
}
```

#### Warning: `--agent-filesystem-root`

**Do not use `--agent-filesystem-root` unless you have a specific reason.** This option replaces the default random temporary directories with deterministic paths. Manually modifying files under this root while agents are running can break durable execution guarantees — Golem relies on controlling the agent filesystem to ensure consistency across restarts and replays. This flag is intended for advanced debugging and inspection scenarios only.

### `golem server clean`

Deletes the local server's data directory without starting the server.

```shell
golem server clean
```

This removes all stored state including deployed components, agent data, and operation logs.

## Important Notes

- **`--clean` deletes all state**: Running `golem server run --clean` deletes all existing agents, deployed components, and data. Never run it without explicitly asking the user for confirmation first.
- **The server runs in the foreground**: It blocks the terminal. Run it in a separate terminal or background process before deploying or invoking agents.
- **Deploy after starting**: Components must be deployed with `golem deploy` after the server is running before agents can be invoked.
- **File limits**: On startup the server automatically attempts to increase the OS file descriptor limit to 1,000,000 for better performance.

## Typical Development Workflow

1. Start the server: `golem server run`
2. In another terminal, deploy: `golem deploy --yes`
3. Invoke agents or use the REPL: `golem repl`
4. After code changes, redeploy: `golem deploy --yes --reset`

## Debugging a Running Server

The server runs in the foreground and writes structured logs to **stderr**. Verbosity is
controlled by the `-v` flag — note that `golem server` defaults to a higher base level than the
rest of the CLI, so the mapping is **different from other `golem` subcommands**:

| Flag                  | `golem server run` level | other `golem ...` commands |
|-----------------------|--------------------------|----------------------------|
| _(no flag, default)_  | INFO                     | ERROR                      |
| `-v`                  | WARN                     | WARN                       |
| `-vv`                 | INFO                     | INFO                       |
| `-vvv`                | DEBUG                    | DEBUG                      |
| `-vvvv`               | TRACE                    | TRACE                      |

Use `-vvv` to see the durability and retry decision logs that are usually needed for diagnosing
runtime behavior:

```shell
golem -vvv server run
```

> **`RUST_LOG` is ignored.** The server builds its own `tracing` filter from the `-v` flag and
> does not consult the `RUST_LOG` environment variable. Use `-v` levels instead.

### Useful Tracing Targets

When diagnosing a specific subsystem you can grep the server's stderr by tracing target.
The most useful prefixes are:

| Prefix                                                      | What it covers                                       |
|-------------------------------------------------------------|------------------------------------------------------|
| `golem_worker_executor::durable_host::http::inline_retry`   | HTTP status-code retry decisions and eligibility     |
| `golem_worker_executor::durable_host::http`                 | All outgoing HTTP host calls and durability events   |
| `golem_worker_executor::durable_host::durability`           | Durable host function replay and retry resolution    |
| `golem_worker_executor::durable_host` (semantic trap retry) | "Semantic trap retry: …" decision lines              |
| `golem_worker_executor::services::events`                   | Internal worker events (invocations, suspends, ...)  |

### Key Log Lines When Diagnosing Common Issues

These are the **first** debug lines to grep for when a feature "doesn't seem to work":

- **Status-code retry not firing for an outgoing HTTP request:**
  ```
  HTTP status retry skipped              reason=<NotIdempotent|BodyNotFinished|NoRetry|...>  uri=...  status=...
  HTTP status retry skipped: inside atomic region
  ```
  The `reason` field is the source of truth — it tells you exactly why a particular request was
  not retried (most commonly `NotIdempotent` for opt-out-of-idempotence cases, or `NoRetry` when
  no `status-code`-keyed policy matched). Look for it before assuming the policy or the feature
  is broken.

- **Semantic trap retry policy decisions:**
  ```
  Semantic trap retry: delaying          retry_policy=...  delay_ms=...  attempt=...  trap=...
  Semantic trap retry: exhausted         retry_policy=...  attempt=...  trap=...
  ```
  Indicates which user-defined retry policy matched the trap and how it decided. Absence of these
  lines for a 5xx-throwing handler means **no** named trap policy matched (the legacy retry
  config is used instead).

- **HTTP retry policy resolution failed (genuine error):**
  ```
  WARN  Failed resolving semantic trap retry policy, falling back to legacy retry config
  ```
  This now only fires for genuine evaluation errors (e.g. type-coercion failures inside a
  predicate). Policies whose predicate references a property that does not exist in the current
  context (e.g. a `status-code`-keyed policy in the trap context) are silently skipped instead.

### Running the Server in the Background

To keep the server running while inspecting logs from another terminal:

```shell
golem -vvv server run > /tmp/golem-server.log 2>&1 &
tail -f /tmp/golem-server.log | grep -E "HTTP status retry|Semantic trap retry"
```
