<!-- golem-managed:guide:go:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Skills

This project includes coding-agent skills in `.agents/skills/`. Load a skill when the task matches its description.

**Activation cues for `golem.yaml` edits**: whenever a task involves editing `golem.yaml`, load `golem-edit-manifest` for the manifest schema, and also load the section-specific skill — `golem-add-env-vars` for `env`/`envDefaults`/`secretDefaults` changes, `golem-add-initial-files` for `files:` blocks, `golem-profiles-and-environments` for `presets`/environment-scoped sections, `golem-manage-plugins` for `plugins:` entries, `golem-configure-api-domain` for `httpApi`, and `golem-configure-mcp-server` for `mcp`.

| Skill | Description |
|-------|-------------|
| `golem-cloud-account-setup` | Setting up a Golem Cloud account — authentication, cloud profiles, API tokens, and first cloud deployment |
| `golem-new-project` | Creating a new Golem application project with `golem new` |
| `golem-add-component` | Adding a new component or agent templates to an existing application |
| `golem-edit-manifest` | Editing the Golem Application Manifest (golem.yaml) — components, agents, templates, environments, httpApi, mcp, bridge SDKs, plugins, and more |
| `golem-build` | Building a Golem application with `golem build` |
| `golem-troubleshoot-build` | Troubleshooting Golem build failures and debugging manifest file (golem.yaml) configuration — diagnosing tool, dependency, env var, config, and manifest layer issues with `golem component manifest-trace` |
| `golem-deploy` | Deploying a Golem application with `golem deploy` |
| `golem-local-dev-server` | Starting, configuring, and debugging the local Golem development server with `golem server` — verbosity flags, useful tracing targets, and key log lines |
| `golem-rollback` | Rolling back a Golem deployment to a previous revision or version |
| `golem-redeploy-agents` | Redeploying existing agents by deleting and recreating them |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins — listing available plugins, installing and configuring plugins via golem.yaml or CLI, and understanding built-in plugins like the OTLP exporter |
| `golem-profiles-and-environments` | Understanding CLI profiles, app environments, and component presets — switching between local/cloud, managing deployment targets, and activating per-environment configuration |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-view-agent-logs` | Viewing agent logs and output via streaming |
| `golem-view-agent-files` | Listing files in an agent's virtual filesystem |
| `golem-list-and-filter-agents` | Listing and querying agents with filters |
| `golem-get-agent-metadata` | Checking agent metadata and status |
| `golem-debug-agent-history` | Querying the operation log |
| `golem-undo-agent-state` | Reverting agent state by undoing operations |
| `golem-interrupt-resume-agent` | Interrupting and resuming a Golem agent |
| `golem-test-crash-recovery` | Simulating a crash on an agent for testing crash recovery |
| `golem-integration-test-setup` | Setting up a dedicated Golem environment for integration testing — isolated local server, test environment in golem.yaml, dynamic port discovery, and non-interactive deploys |
| `golem-cancel-queued-invocation` | Canceling a pending (queued) invocation on an agent |
| `golem-delete-agent` | Deleting an agent instance |

> **Go support is new.** Go-specific skills (`golem-add-agent-go`, `golem-add-http-endpoint-go`, …)
> are not available yet, so only the common skills are listed above. Until they land, use the Go SDK's
> own documentation and the examples under `sdks/go/examples`.

# Golem Application Development Guide (Go)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled to WASM components with `componentize-go` and executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A WASM component compiled from Go, defining one or more agent types
- **Agent type**: A struct annotated with `#derive.agent`, defining the agent's API via its public methods
- **Agent (worker)**: A running instance of an agent type, identified by constructor parameters, with persistent state

## Agent Fundamentals

- Every agent is uniquely identified by its **constructor parameter values** — two agents with the same parameters are the same agent
- Agents are **durable by default** — their state persists across invocations, failures, and restarts
- Invocations are processed **sequentially in a single thread** — no concurrency within a single agent, no need for locks
- Agents can **spawn other agents** and communicate with them via **RPC** (see Agent-to-Agent Communication)
- An agent is created implicitly on first invocation — no separate creation step needed
- **Async handles cannot outlive invocations** — every WASI `pollable` or `future-*` resource (e.g. those returned by `@http.handle`) must be subscribed to / `get()`-ed within the same invocation; do not store unresolved pollables or futures in agent state to consume them from a later invocation

## Durability & Automatic Retries

Golem **automatically retries** failed operations using durable execution. **Do not add manual retry loops, `match` + retry patterns, or backoff utilities in agent code** — let operations fail and Golem will retry them. A built-in default policy (3 retries, exponential backoff with jitter, clamped to [100ms, 1s]) applies when no user-defined policy matches.

The following are retried transparently:

- **HTTP requests** to external services (via `wasi:http` and friends)
- **RPC calls** between agents
- **Database / storage calls** — `golem:rdbms/postgres`, `golem:rdbms/mysql`, `golem:rdbms/ignite2`, `wasi:blobstore`, `wasi:keyvalue`
- **Panics and unhandled errors** (raised via `raise` or propagated with `!`) escaping an agent method — the worker is restarted and the invocation is replayed from the oplog, with all previously-recorded side effects skipped

Only customize when the *strategy* needs to change (different backoff, give-up conditions, per-status-code policies). For that, see the Go SDK's retry-policy helpers.

## Project Structure

```
golem.yaml                        # Root application manifest
go.mod                            # One module for the whole app; pins the SDK and componentize-go
<component>/                      # Component package (each becomes a WASM component)
  golem.yaml                      # Component manifest
  counter.go                      # Agent definition — package main
golem-temp/                       # Build artifacts (gitignored)
```

There is a **single Go module at the app root**. Each component directory is its own `package main`
within that module, so components share dependencies but build to separate WASM components.

## Prerequisites

- Go toolchain: https://go.dev/dl/ (Go 1.25.5 or newer)
- Golem CLI (`golem`) version 1.5.x: https://github.com/golemcloud/golem/releases
- `wasm-tools`: https://github.com/bytecodealliance/wasm-tools

## Name Mapping

Wire names come from the SDK's declarations, not from Go identifiers:

- **Agent type names**: the `Name` in `golem.Spec{Name: "CounterAgent"}`
- **Method names**: the string passed to `golem.DefineMethod[...]("increment")`
- **Parameter and record field names**: the Go field name **lower-camel-cased** — `AmountCents` → `amountCents`
- **Variant case names**: the string in `golem.Case[Card]("card")`
- **Enum case names**: positional, from `golem.DefineEnum[Status]("active", "closed")`

## Key Constraints

- Target is **WASM only** — no raw sockets. `net.Dial`, most database drivers and custom gRPC
  transports cannot work; outgoing HTTP goes through the SDK's WASI-backed transport.
- Bare `int`/`uint` are **rejected** at registration: their width is platform-dependent, so the wire
  type would be ambiguous. Use `int64`/`uint64` or another sized type.
- A `*T` field means `option<T>`. A nil slice or map is an **empty** list/map, never "absent" — spell
  the optional container `*[]T` if you need to distinguish.
- Method descriptors must be **package-level vars**: the same value drives the schema, the
  implementation binding and cross-agent calls.
- Cross-agent calls hang off the descriptor (`Charge.Call(client, in)`), because Go methods cannot
  introduce type parameters.
- `func main() {}` must exist and can be empty — the SDK wires the component exports from its `init()`.
- Multiple agents can coexist in one component; a worker is initialized as exactly one of them.
- Do NOT edit files under `internal/wit/` in the SDK — they are generated.

## Coding Convention

- Standard Go style: `gofmt` decides formatting; `UpperCamelCase` for exported identifiers,
  `lowerCamelCase` otherwise.
- Agents are declared with `golem.DefineAgent`, methods with `golem.DefineMethod`, and behaviour
  bound with `golem.Implement` in an `init()`.
- Handlers may be plain closures or ordinary Go methods bound with a method expression
  (`golem.Bind((*CartState).AddItem)`).

## Tooling

- `gofmt -w .` — format code
- `go vet ./...` — static checks
- `go test ./...` — run tests
- `golem app build` — build the components (runs `go tool componentize-go build`)

## Running Golem CLI commands non-interactively

The `golem` CLI prompts for confirmation when it needs to apply changes such as syncing project skill files, updating dependency configurations, or recreating deployments. In non-interactive contexts (CI, scripts, coding agents) **always pass `--yes` (or `-y`) to mutating commands** so the CLI auto-confirms instead of aborting:

```shell
golem build --yes
golem deploy --yes
golem new --yes --template <LANGUAGE> <APPLICATION_PATH>
golem agent update --yes <AGENT>
```

If you see `This action requires confirmation, but the current shell is non-interactive.` (older CLI versions: `The current input device is not an interactive one, defaulting to "false"`) followed by `Failed to build application`, re-run the same command with `--yes`.

## Documentation

- Golem docs: https://learn.golem.cloud
- Go docs: https://go.dev/doc/
- App manifest reference: https://learn.golem.cloud/app-manifest
<!-- golem-managed:guide:go:end -->
