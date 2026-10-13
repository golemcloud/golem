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
| `--config <FILE>` | TOML configuration file for the server; see [Configuration File](#configuration-file) | _(none)_ |
| `--router-addr <ADDR>` | IPv4 address to serve the main API on | `127.0.0.1` |
| `--router-port <PORT>` | Port to serve the main API on | `9881` |
| `--custom-request-port <PORT>` | Port to serve custom HTTP requests on (HTTP API endpoints) | `9006` |
| `--mcp-port <PORT>` | Port to serve the MCP server on | `9007` |
| `--ports-file <PATH>` | Write discovered startup ports to this JSON file | _(none)_ |
| `--data-dir <PATH>` | Directory to store data in | Platform-specific (see below) |
| `--system-memory-override <SIZE>` | Override detected system memory, e.g. `2GiB`; not a hard RSS limit | Environment variable, config file, manifest, then host/cgroup detection |
| `--clean` | Clean the data directory before starting | `false` |
| `--agent-filesystem-root <PATH>` | Override the default root of the agent filesystems. Layout: `<root>/<environment_id>/<component_id>/<agent_segment>/`; the agent segment is the agent name with each character that is not an ASCII letter, a digit, `-` or `_` replaced by `_`, cut to 32 characters, then `-` and the BLAKE3 hash of the agent id | `<data_dir>/agents` on macOS; temporary directories on other platforms |

On macOS, the local server defaults to `Apfs` storage at `<data_dir>/agents` and enables filesystem snapshots. APFS is for local development and tests only. It provides no per-agent disk limits or filesystem usage metering. Snapshots use blob storage at `<data_dir>/blobs`. The server generates and retains a repository key in `<data_dir>/filesystem-snapshots.repository-key` when no key is configured.

On other platforms, the default storage is `Temporary` and filesystem snapshots are disabled. Supplying `--agent-filesystem-root` selects `Directory` storage there, with filesystem snapshots still disabled.

The filesystem settings are part of the server configuration: `[worker_executor.filesystem_storage]` and `[worker_executor.filesystem_snapshots]` in the `--config` file, or the `GOLEM__WORKER_EXECUTOR__FILESYSTEM_STORAGE__*` and `GOLEM__WORKER_EXECUTOR__FILESYSTEM_SNAPSHOTS__*` environment variables; see [Configuration File](#configuration-file). They override the platform defaults and `localServer.agentFilesystemRoot`, and `--agent-filesystem-root` overrides them. A storage mode that another layer selects does not keep the root of the replaced mode, so give `root` together with `type`. To use `Directory` or `Temporary` on macOS, also set `filesystem_snapshots.type = "Disabled"`.

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

The application manifest can configure the built-in local preset in its `localServer` section. `routerAddr` and `routerPort` define the main API endpoint used both by `golem server run` and by other `golem` commands that target the built-in local preset. `customRequestPort` and `mcpPort` control how deployment `subdomain` values expand: HTTP API domains resolve to `<label>.localhost:<customRequestPort>` and MCP domains resolve to `<label>.localhost:<mcpPort>`. Requests are routed by matching their `Host` header against that expansion, so the server has to bind the same ports: `golem server run` warns when its `--custom-request-port` or `--mcp-port` (including `0`) differs from what the manifest expands to, and those deployments are not reachable until the values match. Use nonzero ports for deployments that register persistent subdomains. Load `golem-configure-api-domain` or `golem-configure-mcp-server` for the full `subdomain` versus `domain` rules.

#### Memory Budget

Override detected system memory to leave room for other applications and control how much memory a local server uses for agent admission and eviction. This is useful for everyday development, smaller machines, and running multiple servers. Without an override, Golem detects host or cgroup memory; on macOS, a local server sees the full host RAM.

```shell
golem server run --system-memory-override 2GiB
GOLEM__WORKER_EXECUTOR__MEMORY__SYSTEM_MEMORY_OVERRIDE=2147483648 golem server run
```

The flag and the manifest field use the same positive byte-size format. Units are case-insensitive: `1mb` and `1MB` mean 1,000,000 bytes; `1MiB` means 1,048,576 bytes. Whole-number quantities and ASCII spaces are accepted; use `1536 MiB` instead of `1.5 GiB`. A value without a unit means bytes. Quote unitless values in YAML. The environment variable and the config file field `worker_executor.memory.system_memory_override` take a number of bytes. Precedence is the `--system-memory-override` flag, then the environment variable, then the config file, then `localServer.systemMemoryOverride` in the manifest, then automatic detection. All of them set the executor's `memory.system_memory_override`. The executor uses 80% for agent admission and eviction, leaving 20% for host overhead. This is not an OS-enforced process RSS limit; compilation and other host allocations can exceed it. Check the startup log's measured memory limit and monitor RSS under load.

```yaml
localServer:
  systemMemoryOverride: 2GiB
```

#### Manual Testing with Free Ports

For manual testing in temporary apps, pass `0` directly as `golem server run` flags to let the OS assign free ports:

```shell
golem server run \
  --router-port 0 \
  --custom-request-port 0 \
  --mcp-port 0 \
  --ports-file .golem/ports.json \
  --data-dir .golem/data
```

The `0` flag values request OS-assigned free ports. The `--ports-file` path records the actual bound ports and is also the readiness signal; wait for `.golem/ports.json` before sending requests.

Do not put `0` in manifest `localServer` port fields. Manifest `localServer.customRequestPort` and `localServer.mcpPort` values are used for deployment `subdomain` expansion and must be stable nonzero ports.

For persistent local deployment subdomains, use stable manifest ports or omit the port fields to use the defaults:

```yaml
localServer:
  routerAddr: 127.0.0.1
  routerPort: 9881
  customRequestPort: 9006
  mcpPort: 9007
  portsFile: .golem/ports.json
  dataDir: .golem/data
```

`routerAddr` and `routerPort` define where the built-in local preset is reachable: `golem server run` binds the server there, and other `golem` commands connect there. `routerAddr` must be an IPv4 address literal; host names are not supported. When the server binds to `0.0.0.0`, commands connect through `127.0.0.1`. The server refuses to bind the main API to an address that is not loopback, such as `0.0.0.0`, while the configuration uses a well-known default secret; see [Configuration File](#configuration-file). `routerAddr` affects only the main API: the HTTP API and MCP listeners bind `worker_service.bind_address`, which is `127.0.0.1` by default, and deployment subdomains always expand to `<label>.localhost:<port>`, which resolves only on the machine running the server. For access from other machines, use `domain` instead of `subdomain`. Setting only `portsFile` records the selected ports but does not request free ports. Explicit CLI flags such as `--router-port`, `--custom-request-port`, `--mcp-port`, `--ports-file`, and `--data-dir` override the manifest values for `server run`; overriding `--custom-request-port` or `--mcp-port` does not change the subdomain expansion, so keep them equal to the manifest values when subdomain deployments are used.

#### Configuration File

`golem server run --config <FILE>` reads a TOML file that configures every service of the server. Without it, the server uses the local development setup: SQLite databases and blobs in the data directory, login disabled, an unlimited plan, and the well-known local admin token that other `golem` commands use for the built-in local preset.

The file has the top-level fields `data_dir` and `ports_file`, a `[router]` section with the `address` and `port` of the main API, and one section per service. Each service section has the same shape as that service's own configuration file:

| Section | Service |
|---------|---------|
| `[registry_service]` | Registry service: database, login, initial accounts and plans, blob storage |
| `[shard_manager]` | Shard manager: persistence |
| `[worker_service]` | Worker service: custom request and MCP ports, session storage, blob storage |
| `[worker_executor]` | Worker executor: key-value, indexed and scheduler storage, blob storage, memory, agent filesystems, resource usage metering |
| `[component_compilation_service]` | Component compilation service: blob storage |

`GOLEM__<SECTION>__<FIELD>` environment variables override the file; for example, `GOLEM__WORKER_EXECUTOR__RESOURCE_USAGE_METERING__COMPUTE=true`. The `server run` flags override both, and the file overrides the manifest's `localServer` section. Every field has a default, so a file only needs the fields it changes. An enum-valued field selects its variant with `type` and its settings with `config`; selecting another `type` than the default drops the default variant's settings.

`golem --dump-config-default-toml` prints the default configuration, followed by a commented example that keeps all state in Postgres and the blobs in S3. It leaves out the settings that the server sets itself and the paths that follow `data_dir`, so it is a valid config file in which changing `data_dir` moves all local state. `golem --dump-config-default-env-var` prints the same as environment variables.

The server sets the ports, the hosts that connect the services to each other, and the tracing of every service itself. It rejects a file or an environment variable that sets one of them, for example `worker_executor.grpc.port` or `registry_service.tracing`. It also rejects unknown top-level fields and unknown fields in `[router]`. The main API applies `registry_service.cors_origin_regex` to both the registry and the worker service APIs, so `worker_service.cors_origin_regex` is rejected too. The value is a pattern in which `*` matches any characters and `?` matches one character, for example `https://*.example.com`. The default value `.*` matches no browser origin, so browsers cannot call the API from another origin.

Map entries and fields that have a default value cannot be removed, only changed. For example, the `root` initial account and the `default` plan always exist; with OAuth2 login, set the `root` token to a secret value.

The default configuration uses secrets that are public: the well-known local admin token, which other `golem` commands use for the built-in local preset, and the compiled-in keys that sign webhook URLs and invocation session tokens. To serve the main API, the HTTP APIs or MCP to other machines, replace all of them. A token has at least 16 characters, each an ASCII letter, a digit, `-` or `_`. Every key is base64, and the two webhook keys must be equal:

```toml
[router]
address = "0.0.0.0"

[worker_service]
bind_address = "0.0.0.0"

[registry_service.initial_accounts.root]
token = "<a long random secret>"

[worker_service.webhook_callback_handler]
hmac_key = "<32 random bytes, base64>"

[worker_executor.agent_webhooks_service]
hmac_key = "<the same 32 random bytes, base64>"

[worker_service.invocation_session_tokens.active_key]
id = "<a key id>"
key = "<32 random bytes, base64>"
```

The server refuses to bind any listener to an address that is not loopback while any of these settings has its well-known default value, and lists the listeners and the settings to change. The listener addresses are `router.address` and the `bind_address` of every service. The check covers every listener, not only the one that verifies a given secret. When no initial account has the well-known admin token, the server deletes that token from the registry at startup, so a data directory that was used with the default configuration stops accepting it. Pass the new admin token to other `golem` commands with a profile, or with the `GOLEM_AUTH_TOKEN` environment variable; `golem repl` with the built-in local preset always uses the well-known token, so use a profile for it. Other `golem` commands find the server through the manifest's `localServer` section or a profile, not through the server's config file.

Replace the secrets also when the main API binds a loopback address but a reverse proxy on the same host serves it to other machines; the check cannot detect a proxy.

Every service binds all its listeners to its `bind_address`, which is `127.0.0.1` by default. `0.0.0.0` accepts IPv4 only, and `::` accepts both IPv6 and IPv4. The custom request and MCP listeners belong to the worker service, so serving HTTP APIs or MCP to other machines needs `worker_service.bind_address`. That address also binds the gRPC listener of the worker service, on a port that the operating system selects, and the gRPC listeners do not authenticate their callers. Keep the `bind_address` of the other services at `127.0.0.1`, and use a firewall so that other machines reach only the main API, the custom request port and the MCP port.

To move the state to Postgres, set the database of each service. Each of them needs its own schema, and the server rejects two that share one. Use the schemas of the per-service deployment, so the same database also works with the per-service images later:

| Field | Schema |
|-------|--------|
| `registry_service.db` | `golem_registry_service` |
| `shard_manager.persistence` | `golem_shard_manager` |
| `worker_executor.key_value_storage` | `golem_worker_executor` |
| `worker_executor.indexed_storage` | `golem_worker_executor_indexed` |
| `worker_executor.scheduler_storage` | `golem_worker_executor_scheduler` |

```toml
[registry_service.db]
type = "Postgres"

[registry_service.db.config]
host = "db.internal"
port = 5432
database = "golem"
username = "golem"
password = "<password>"
max_connections = 10
schema = "golem_registry_service"
```

Set `blob_storage` in the `registry_service`, `worker_service`, `worker_executor` and `component_compilation_service` sections to the same value, for example `type = "S3"`. Plan quotas are in `[registry_service.initial_plans.default]`, and OAuth2 login is `[registry_service.login]`.

In the default SQLite setup, all state is in the data directory: `registry.db`, `shard_manager.db`, `scheduler.db` and `gateway-sessions.db`, one database per agent in `kv-store/*.db`, and the blobs in `blobs/`.

#### Warning: `--agent-filesystem-root`

**Do not use `--agent-filesystem-root` unless you have a specific reason.** On macOS, the local server already keeps agent files on `Apfs` storage at `<data_dir>/agents`, and this option moves that root. The new root must be on a volume that can clone files, or the server fails to start. On other platforms, this option replaces the default random temporary directories with deterministic paths. Manually modifying files under this root while agents are running can break durable execution guarantees — Golem relies on controlling the agent filesystem to ensure consistency across restarts and replays. This flag is intended for advanced debugging and inspection scenarios only.

### `golem server clean`

Deletes the local server's data directory without starting the server.

```shell
golem server clean
```

This removes all stored state including deployed components, agent data, and operation logs.

This removes the data directory that `golem server run` uses with the same configuration: `data_dir` from the `--config` file or the `GOLEM__DATA_DIR` environment variable, then `localServer.dataDir` of a discovered manifest, then the platform default data directory. It refuses when a store keeps state outside the data directory, for example a Postgres database, S3 blob storage, or a path that leaves the data directory through `..` or a symlink, because deleting the directory would not delete the server state. It also refuses when the data directory itself is a symlink, because deleting a symlink does not delete its target. `golem server run --clean` refuses in the same cases. Pass `-X` to ignore a discovered manifest. The CLI displays the resolved absolute path and asks for confirmation before deleting it; filesystem roots are always rejected.

## Important Notes

- **Cleanup deletes all state**: Both `golem server clean` and `golem server run --clean` delete all existing agents, deployed components, and data from the resolved directory. Never run either command without explicitly asking the user for confirmation first. In a non-interactive workflow, pass `--yes` only after the user approves the exact target path.
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
  Indicates which named retry policy matched a retryable trap and how it decided. A deterministic
  trap outside an atomic region is not retried, so it does not produce these policy decisions.
  For a retryable trap, policies that require an absent property (such as `status-code` in the
  trap context) are skipped; the synthesized default policy provides the fallback.

- **HTTP retry policy resolution failed (genuine error):**
  ```
  WARN  Failed resolving semantic trap retry policy; giving up
  ```
  This fires for policy resolution errors (e.g. type-coercion failures inside a predicate) and
  stops the retry. Policies whose predicate requires a property that does not exist in the
  current context (e.g. a `status-code`-keyed policy in the trap context) are skipped instead.

### Running the Server in the Background

To keep the server running while inspecting logs from another terminal:

```shell
golem -vvv server run > /tmp/golem-server.log 2>&1 &
tail -f /tmp/golem-server.log | grep -E "HTTP status retry|Semantic trap retry"
```
