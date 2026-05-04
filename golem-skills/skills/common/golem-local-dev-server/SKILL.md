---
name: golem-local-dev-server
description: "Starting and managing the local Golem development server with `golem server`. Use when asked to start, stop, clean, or configure the local Golem server."
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
