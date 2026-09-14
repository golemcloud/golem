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
| `golem-add-agent-go` | Adding a new Go agent to a Golem component |
| `golem-annotate-agent-go` | Configuring an agent's definition — name, description, mode, HTTP mount, method descriptions |
| `golem-multi-instance-agent-go` | Addressing multiple instances of an agent by identity (and phantoms) |
| `golem-stateless-agent-go` | Creating ephemeral (stateless) agents |
| `golem-configure-durability-go` | Choosing durable vs ephemeral agents, and adding periodic snapshots |
| `golem-custom-snapshot-go` | Snapshot-based recovery and customizing state save/load |
| `golem-call-another-agent-go` | Calling one agent from another via typed RPC |
| `golem-fire-and-forget-go` | Fire-and-forget agent invocations with `Trigger` |
| `golem-parallel-workers-go` | Fanning out work to parallel agents and collecting results |
| `golem-recurring-task-go` | Recurring / scheduled work (self-rescheduling via `Schedule`) |
| `golem-schedule-future-call-go` | Scheduling a one-off future call from code, and canceling it |
| `golem-wait-for-external-input-go` | Waiting for external input using Golem promises |
| `golem-call-from-external-go` | Invoking agents from outside the platform (CLI, HTTP, worker REST API) |
| `golem-add-http-endpoint-go` | Exposing an agent's methods over HTTP |
| `golem-http-params-go` | Mapping HTTP path/query/header/body to method inputs |
| `golem-add-http-auth-go` | Requiring authentication on HTTP endpoints |
| `golem-add-cors-go` | Configuring CORS on HTTP endpoints |
| `golem-add-webhook-go` | Receiving external webhook callbacks |
| `golem-make-http-request-go` | Making outgoing HTTP requests via `net/http` |
| `golem-mark-read-only-go` | Marking methods read-only (side-effect-free) with result caching |
| `golem-atomic-block-go` | Atomic regions, custom durability (`DurableOp`), idempotence, oplog commit, idempotency keys |
| `golem-retry-policies-go` | Configuring semantic retry policies |
| `golem-add-transactions-go` | Saga-pattern transactions with compensation |
| `golem-add-postgres-go` | Using PostgreSQL via the `golem/rdbms/postgres` wrapper |
| `golem-add-mysql-go` | Using MySQL via the `golem/rdbms/mysql` wrapper |
| `golem-add-config-go` | Adding typed configuration (`DefineConfiguredAgent` + `golem.Config`) |
| `golem-add-secret-go` | Adding typed secrets (`golem.Secret[T]`) |
| `golem-file-io-go` | Reading and writing files with the standard `os`/`io` packages |
| `golem-logging-go` | Structured logging via `log/slog` |
| `golem-enable-otlp-go` | Enabling the OpenTelemetry (OTLP) exporter plugin |
| `golem-invoke-agent-go` | Invoking an agent method from the CLI and waiting for the result |
| `golem-trigger-agent-go` | Triggering a fire-and-forget invocation from the CLI |
| `golem-schedule-agent-go` | Scheduling a future invocation from the CLI |
| `golem-create-agent-instance-go` | Creating an agent instance with `golem agent new` |
| `golem-interactive-repl-go` | Interactive testing/scripting of agents via the REPL |
| `golem-add-go-module` | Adding a Go module dependency |

# Golem Application Development Guide (Go)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled to WASM components with `componentize-go` and executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A WASM component compiled from Go, defining one or more agent types
- **Agent type**: A state-free **definition** (`golem.DefineAgent` + `golem.DefineMethod` descriptors) plus an **implementation** (`golem.Implement` + `golem.Handle`), split across a `<name>` / `impl` package pair
- **Agent (worker)**: A running instance of an agent type, identified by constructor parameters, with persistent state

## Agent Fundamentals

- Every agent is uniquely identified by its **constructor parameter values** — two agents with the same parameters are the same agent
- Agents are **durable by default** — their state persists across invocations, failures, and restarts
- Invocations are processed **sequentially in a single thread** — no concurrency within a single agent, no need for locks
- Agents can **spawn other agents** and communicate with them via **RPC** (see the `golem-call-another-agent-go` skill)
- An agent is created implicitly on first invocation — no separate creation step needed
- **Async handles cannot outlive invocations** — every async host resource (e.g. an outgoing-HTTP response or an RPC `*golem.Future`) must be consumed within the same invocation; do not store an unresolved response or future in agent state to consume it from a later invocation

## Durability & Automatic Retries

Golem **automatically retries** failed operations using durable execution. **Do not add manual retry loops, `match` + retry patterns, or backoff utilities in agent code** — let operations fail and Golem will retry them. A built-in default policy (3 retries, exponential backoff with jitter, clamped to [100ms, 1s]) applies when no user-defined policy matches.

The following are retried transparently:

- **HTTP requests** to external services (via `wasi:http` and friends)
- **RPC calls** between agents
- **Database / storage calls** — `golem:rdbms/postgres`, `golem:rdbms/mysql`, `wasi:blobstore`, `wasi:keyvalue`
- **Panics** escaping an agent method (e.g. from `golem.Must` on an unexpected error) — the worker is restarted and the invocation is replayed from the oplog, with all previously-recorded side effects skipped

Only customize when the *strategy* needs to change (different backoff, give-up conditions, per-status-code policies). For that, see the Go SDK's retry-policy helpers.

## Project Structure

A single-component app keeps the whole Go module in a `module/` directory:

```
golem.yaml                        # Application manifest
module/                           # This component — its own Go module (golem.yaml dir: "module")
  go.mod                          # pins the SDK and componentize-go
  main.go                         # package main — blank-imports the SDK + each agent's impl package
  agents/                         # one folder per agent
    counter/                      #   the counter agent
      counter.go                  #     package counter — DEFINITION: DefineAgent + method descriptors + types
      impl/
        impl.go                   #     package impl — IMPLEMENTATION: golem.Implement (registers on import)
  internal/                       # (optional) any non-agent packages
golem-temp/                       # Build artifacts (gitignored)
```

The Go module lives in its own directory rather than at the app root, so the module — `go.mod` plus all
your packages — stays sealed away from the application's `golem.yaml` and the `golem-temp/` build output
(a `go.mod` at the app root would otherwise enclose them). The directory name never appears in import
paths (`go.mod` is the module root), so imports stay clean: `<module>/agents/counter`.

Adding a **second** component promotes the app to the multi-component layout: `module/` is renamed to its
component name and each further component gets its own directory (each still its own Go module):

```
golem.yaml
<component-a>/                    # was module/, renamed on promotion
  go.mod  main.go  agents/ ...
<component-b>/
  go.mod  main.go  agents/ ...
golem-temp/
```

Within a component, each agent lives in `agents/<name>/`, split across two packages: a state-free
**definition** (`package <name>`, e.g. `counter`) holding the identity, method descriptors, and
input/output types; and an **implementation** in the nested `impl/` subpackage holding the private state
and the handlers. This split is what lets agents call one another without a Go import cycle — a caller
imports only the callee's definition package (`<module>/agents/ledger`, used as `ledger.Agent`,
`ledger.Record`), never its implementation. `main.go` (`package main`) blank-imports the SDK and each
agent's **impl** package so their registration runs on import — the same blank-import-for-side-effects
pattern Go uses for database drivers (`import _ "..."`). Adding an agent means adding an `agents/<name>/`
folder (a def file + an `impl/` subpackage) and a blank import to `main.go`. Non-agent packages can live
anywhere in the module (e.g. an `internal/` directory).

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
- Method descriptors must be **package-level vars** in the definition package: the same value drives
  the schema, the implementation binding and cross-agent calls.
- State is **private to the implementation package** — a caller never sees it. Cross-agent calls go
  through the definition: `client := ledger.Agent.Get(id)` then `ledger.Record.Call(client, in)`.
  The client hangs off the definition and the call off the descriptor, because Go methods cannot
  introduce type parameters.
- `main.go` (`package main`) holds an empty `func main() {}` plus a blank import of each agent's
  **implementation** package. The blank `_ "github.com/golemcloud/golem/sdks/go/golem"` import must
  stay — it initializes the SDK runtime (e.g. HTTP) and links the component exports; do not remove it
  even though it looks unused.
- Multiple agents can coexist in one component (each split into a definition + implementation package,
  the impls all blank-imported by `main.go`); a worker is initialized as exactly one of them.
- **Environment variables** use the standard library — `os.Getenv` / `os.Environ` read the worker's
  environment with no special setup (they route to `wasi:cli/environment` via the toolchain's WASI
  adapter). Alongside any vars you set (`env:` in `golem.yaml`, or `golem agent new --env`), the runtime
  injects `GOLEM_AGENT_ID`, `GOLEM_WORKER_NAME`, `GOLEM_COMPONENT_ID`, `GOLEM_COMPONENT_REVISION` and
  `GOLEM_AGENT_TYPE`.
- **Logging** uses the standard library — the SDK installs an `slog` handler on startup that routes
  `slog` (and, via slog, the standard `log` package) through the host logging channel, so records carry a
  real level and context in worker logs (`slog.Info(...)` → an `INFO` event, `slog.Warn`/`Error`
  likewise). Plain `fmt.Println` / direct `os.Stdout`/`os.Stderr` writes are still captured, but as raw
  stdout/stderr with no level. Tune it with `golem/log`'s `SetDefault(&log.Options{Level: ...})`; view
  output with the `golem-view-agent-logs` skill.
- Do NOT edit files under `internal/wit/` in the SDK — they are generated.

## Coding Convention

- Standard Go style: `gofmt` decides formatting; `UpperCamelCase` for exported identifiers,
  `lowerCamelCase` otherwise.
- Agents are declared with `golem.DefineAgent` and their methods with `golem.DefineMethod` (in the
  definition package); behaviour is attached in the implementation package with
  `impl := golem.Implement(def, init)` and then `golem.Handle(impl, method, handler)` per method (in an
  `init()`). A configured agent whose constructor reads config uses `golem.ImplementConfigured` and reads
  config in a method via `golem.Config(def, ctx)`.
- Handlers may be plain closures or ordinary Go methods bound with a method expression
  (`golem.Handle(impl, Cart.AddItem, golem.Bind((*state).AddItem))`).

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
