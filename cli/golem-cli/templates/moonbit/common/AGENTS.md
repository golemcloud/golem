<!-- golem-managed:guide:moonbit:start -->
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
| `golem-create-agent-instance-moonbit` | Creating a new agent instance with `golem agent new` |
| `golem-invoke-agent-moonbit` | Invoking a Golem agent method from the CLI |
| `golem-trigger-agent-moonbit` | Triggering a fire-and-forget invocation on a Golem agent |
| `golem-schedule-agent-moonbit` | Scheduling a future invocation on a Golem agent |
| `golem-add-moonbit-package` | Adding a MoonBit mooncakes dependency to the project |
| `golem-add-postgres-moonbit` | Connecting to PostgreSQL with `golem:rdbms/postgres` from MoonBit agents |
| `golem-add-mysql-moonbit` | Connecting to MySQL with `golem:rdbms/mysql` from MoonBit agents |
| `golem-add-ignite-moonbit` | Connecting to Apache Ignite 2 with `golem:rdbms/ignite2` from MoonBit agents |
| `golem-add-agent-moonbit` | Adding a new agent type to a MoonBit Golem component |
| `golem-stateless-agent-moonbit` | Creating ephemeral (stateless) agents with a fresh instance per invocation |
| `golem-annotate-agent-moonbit` | Adding prompt and description annotations to agent methods |
| `golem-mark-read-only-moonbit` | Marking agent methods as read-only for a side-effect-free guarantee, result caching, and HTTP cache headers |
| `golem-configure-durability-moonbit` | Choosing between durable and ephemeral agents |
| `golem-call-another-agent-moonbit` | Calling another agent and awaiting the result (RPC) |
| `golem-call-from-external-moonbit` | Calling agents from external applications (no bridge generator yet — use the REST API or a TS/Rust bridge) |
| `golem-fire-and-forget-moonbit` | Triggering an agent invocation without waiting for the result |
| `golem-parallel-workers-moonbit` | Fan out work to multiple parallel agents and collect results |
| `golem-schedule-future-call-moonbit` | Scheduling a future agent invocation from within agent code |
| `golem-recurring-task-moonbit` | Implementing recurring (cron-like) tasks via self-scheduling — periodic polling, cleanup, heartbeats, backoff, and cancellation |
| `golem-wait-for-external-input-moonbit` | Waiting for external input using Golem promises (human-in-the-loop, webhooks, external events) |
| `golem-add-webhook-moonbit` | Creating and awaiting webhooks for integrating with webhook-driven external APIs |
| `golem-multi-instance-agent-moonbit` | Creating multiple agent instances with the same constructor parameters using phantom agents |
| `golem-atomic-block-moonbit` | Atomic blocks, persistence control, and idempotency |
| `golem-add-transactions-moonbit` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-moonbit` | Exposing an agent over HTTP with mount paths and endpoint annotations |
| `golem-http-params-moonbit` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-moonbit` | Enabling authentication on HTTP endpoints |
| `golem-add-cors-moonbit` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins — listing available plugins, installing and configuring plugins via golem.yaml or CLI, and understanding built-in plugins like the OTLP exporter |
| `golem-add-config-moonbit` | Adding typed configuration to a MoonBit Golem agent |
| `golem-add-secret-moonbit` | Adding secrets to MoonBit Golem agents |
| `golem-quota-moonbit` | Adding resource quotas (rate limiting, capacity, concurrency) to MoonBit Golem agents using QuotaToken and reservations |
| `golem-retry-policies-moonbit` | Configuring semantic retry policies — composable exponential/periodic/fibonacci backoff, predicates on error properties, scoped overrides with `with_named_policy!`, and live CLI management |
| `golem-profiles-and-environments` | Understanding CLI profiles, app environments, and component presets — switching between local/cloud, managing deployment targets, and activating per-environment configuration |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-file-io-moonbit` | Reading and writing files from agent code |
| `golem-add-llm-moonbit` | Adding LLM and AI capabilities by calling provider APIs with WASI HTTP |
| `golem-make-http-request-moonbit` | Making outgoing HTTP requests from agent code |
| `golem-logging-moonbit` | Adding logging to a MoonBit Golem agent using the `@logging` module and `wasi:logging` |
| `golem-enable-otlp-moonbit` | Enabling the OpenTelemetry (OTLP) plugin for a MoonBit agent — exporting traces, logs, and metrics to an OTLP collector, adding custom spans with the `@context` API |
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
| `golem-interactive-repl-moonbit` | Using the Golem REPL for interactive testing and scripting of agents |

# Golem Application Development Guide (MoonBit)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled to WASM using the MoonBit compiler and executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A WASM module compiled from MoonBit, defining one or more agent types
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

Only customize when the *strategy* needs to change (different backoff, give-up conditions, per-status-code policies). For that, see the `golem-retry-policies-moonbit` skill.

## Project Structure

```
golem.yaml                        # Root application manifest
moon.mod.json                     # Module definition (deps, preferred-target: wasm)
moon.pkg                          # Root package config
<component>/                      # Component package (each becomes a WASM component)
  moon.pkg                        # Package config (imports, is-main, link exports)
  counter.mbt                     # Agent definition
  golem_reexports.mbt             # Generated — re-exports WASM entry points from SDK
  golem_agents.mbt                # Generated — agent registration and RawAgent dispatch
  golem_derive.mbt                # Generated — serialization impls for custom types
  golem_clients.mbt               # Generated — RPC client stubs for all agents
golem-temp/                       # Build artifacts (gitignored)
```

## Prerequisites

- MoonBit toolchain (`moon`): https://docs.moonbitlang.com
- Golem CLI (`golem`) version 1.5.x: https://github.com/golemcloud/golem/releases
- `wasm-tools`: https://github.com/bytecodealliance/wasm-tools

## Name Mapping

All MoonBit identifiers are used **as-is** (matching the source code) when used externally in CLI commands, REPL, and agent IDs:

- **Agent type names**: `CounterAgent` → `CounterAgent`, `TaskManager` → `TaskManager` (PascalCase)
- **Method names**: `get_value` → `get_value`, `add_task` → `add_task` (snake_case)
- **Record field names**: `field_name` → `field_name`
- **Enum/variant case names**: `High` → `High`, `Low` → `Low` (PascalCase)

## Key Constraints

- Target is **WASM only** — no native system calls, threads, or platform-specific code
- String encoding is **UTF-16** (MoonBit's native format)
- All agent method parameters are passed by value
- All custom types need `#derive.golem_schema` (which generates `HasElementSchema`, `FromExtractor`, `FromElementValue`, `ToElementValue` impls)
- Do NOT manually edit generated files (`golem_reexports.mbt`, `golem_agents.mbt`, `golem_derive.mbt`, `golem_clients.mbt`)
- Do NOT manually edit files in `wit/` directories — they are managed by the SDK
- `golem-temp/` and `_build/` are gitignored build artifacts
- The `fn main {}` block must exist in the main package (can be empty)
- Multiple agents can coexist in the same package

## Coding Convention

- MoonBit code is organized in block style, each block is separated by `///|`; the order of blocks is irrelevant
- Follow existing naming: `snake_case` for functions/values, `UpperCamelCase` for types/enums
- Keep deprecated blocks in a file called `deprecated.mbt`

## Tooling

- `moon fmt` — format code
- `moon check --target wasm` — type-check (must target WASM)
- `moon test` — run tests; use `moon test --update` to update snapshots
- `moon info` — regenerate `.mbti` interface files
- Always run `moon info && moon fmt` before finalizing changes

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
- MoonBit docs: https://docs.moonbitlang.com
- App manifest reference: https://learn.golem.cloud/app-manifest
<!-- golem-managed:guide:moonbit:end -->
