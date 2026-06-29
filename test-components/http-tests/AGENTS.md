<!-- golem-managed:guide:rust:start -->
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
| `golem-create-agent-instance-rust` | Creating a new agent instance with `golem agent new` |
| `golem-invoke-agent-rust` | Invoking a Golem agent method from the CLI |
| `golem-trigger-agent-rust` | Triggering a fire-and-forget invocation on a Golem agent |
| `golem-schedule-agent-rust` | Scheduling a future invocation on a Golem agent |
| `golem-add-rust-crate` | Adding a Rust crate dependency to the project |
| `golem-add-postgres-rust` | Connecting to PostgreSQL with `golem:rdbms/postgres` from Rust agents |
| `golem-add-mysql-rust` | Connecting to MySQL with `golem:rdbms/mysql` from Rust agents |
| `golem-add-ignite-rust` | Connecting to Apache Ignite 2 with `golem:rdbms/ignite2` from Rust agents |
| `golem-add-agent-rust` | Adding a new agent type to a Rust Golem component |
| `golem-configure-durability-rust` | Choosing between durable and ephemeral agents |
| `golem-stateless-agent-rust` | Creating ephemeral (stateless) agents with a fresh instance per invocation |
| `golem-annotate-agent-rust` | Adding prompt and description annotations to agent methods |
| `golem-call-another-agent-rust` | Calling another agent and awaiting the result (RPC) |
| `golem-call-from-external-rust` | Calling agents from external Rust applications using generated bridge SDKs |
| `golem-fire-and-forget-rust` | Triggering an agent invocation without waiting for the result |
| `golem-parallel-workers-rust` | Fan out work to multiple parallel agents and collect results |
| `golem-schedule-future-call-rust` | Scheduling a future agent invocation |
| `golem-recurring-task-rust` | Implementing recurring (cron-like) tasks via self-scheduling — periodic polling, cleanup, heartbeats, backoff, and cancellation |
| `golem-wait-for-external-input-rust` | Waiting for external input using Golem promises (human-in-the-loop, webhooks, external events) |
| `golem-add-webhook-rust` | Creating and awaiting webhooks for integrating with webhook-driven external APIs |
| `golem-multi-instance-agent-rust` | Creating multiple agent instances with the same constructor parameters using phantom agents |
| `golem-atomic-block-rust` | Atomic blocks, persistence control, and idempotency |
| `golem-add-transactions-rust` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-rust` | Exposing an agent over HTTP with mount paths and endpoint annotations |
| `golem-http-params-rust` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-rust` | Enabling authentication on HTTP endpoints |
| `golem-add-cors-rust` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins — listing available plugins, installing and configuring plugins via golem.yaml or CLI, and understanding built-in plugins like the OTLP exporter |
| `golem-add-config-rust` | Adding typed configuration to a Rust Golem agent |
| `golem-add-secret-rust` | Adding secrets to Rust Golem agents |
| `golem-quota-rust` | Adding resource quotas (rate limiting, capacity, concurrency) to Rust Golem agents using QuotaToken and reservations |
| `golem-retry-policies-rust` | Configuring semantic retry policies — composable exponential/periodic/fibonacci backoff, predicates on error properties, scoped overrides with `with_named_policy`, and live CLI management |
| `golem-profiles-and-environments` | Understanding CLI profiles, app environments, and component presets — switching between local/cloud, managing deployment targets, and activating per-environment configuration |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-file-io-rust` | Reading and writing files from agent code |
| `golem-add-llm-rust` | Adding LLM and AI capabilities using golem-ai libraries |
| `golem-make-http-request-rust` | Making outgoing HTTP requests from agent code using wstd |
| `golem-logging-rust` | Adding logging to a Rust Golem agent using the `log` crate |
| `golem-enable-otlp-rust` | Enabling the OpenTelemetry (OTLP) plugin for a Rust agent — exporting traces, logs, and metrics to an OTLP collector, adding custom spans with the invocation context API |
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
| `golem-interactive-repl-rust` | Using the Golem REPL for interactive testing and scripting of agents |

# Golem Application Development Guide (Rust)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled to `wasm32-wasip2` and executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A WASM module compiled from Rust, defining one or more agent types
- **Agent type**: A trait annotated with `#[agent_definition]`, defining the agent's API
- **Agent (worker)**: A running instance of an agent type, identified by constructor parameters, with persistent state

## Agent Fundamentals

- Every agent is uniquely identified by its **constructor parameter values** — two agents with the same parameters are the same agent
- Agents are **durable by default** — their state persists across invocations, failures, and restarts
- Invocations are processed **sequentially in a single thread** — no concurrency within a single agent, no need for locks
- Agents can **spawn other agents** and communicate with them via **RPC** (see Agent-to-Agent Communication)
- An agent is created implicitly on first invocation — no separate creation step needed
- **Futures cannot outlive invocations** — every `Future` spawned during an invocation must complete (be `.await`ed or driven to completion) before the invocation returns; do not store unresolved futures in agent state to poll them from a later invocation

## Durability & Automatic Retries

Golem **automatically retries** failed operations using durable execution. **Do not add manual retry loops, `loop { match ... }` retry patterns, or backoff utilities in agent code** — let operations fail and Golem will retry them. A built-in default policy (3 retries, exponential backoff with jitter, clamped to [100ms, 1s]) applies when no user-defined policy matches.

The following are retried transparently:

- **HTTP requests** to external services (via `wstd::http`, `golem-wasi-http`, `wasi:http`, etc.)
- **RPC calls** between agents
- **Database / storage calls** — `golem:rdbms/postgres`, `golem:rdbms/mysql`, `golem:rdbms/ignite2`, `wasi:blobstore`, `wasi:keyvalue`
- **Panics** at the top level of an agent method — the worker is restarted and the invocation is replayed from the oplog, with all previously-recorded side effects skipped

Only customize when the *strategy* needs to change (different backoff, give-up conditions, per-status-code policies). For that, see the `golem-retry-policies-rust` skill.

## Project Structure

```
# Single-component app
golem.yaml                        # Golem Application Manifest (contains components.<name>.dir = ".")
Cargo.toml                        # Component crate manifest
src/
  lib.rs                          # Module entry point; re-exports of agents
  <agent_name>.rs                 # Agent definitions and implementations

# Multi-component app
golem.yaml                        # Golem Application Manifest (components map with explicit dir per component)
<component-a>/
  Cargo.toml                      # Component crate manifest (must use crate-type = ["cdylib"])
  src/
    lib.rs                        # Module entry point; re-exports of agents
    <agent_name>.rs               # Agent definitions and implementations
<component-b>/
  Cargo.toml                      # Component crate manifest (must use crate-type = ["cdylib"])
  src/
    lib.rs                        # Module entry point; re-exports of agents
    <agent_name>.rs               # Agent definitions and implementations

golem-temp/                       # Build artifacts (gitignored)
  common/                         # Shared Golem templates (generated on-demand)
    rust/                         # Shared Golem Rust templates
      golem.yaml                  # Build templates for all Rust components
```

## Prerequisites

- Rust with `wasm32-wasip2` target: `rustup target add wasm32-wasip2`
- Golem CLI (`golem`): download from https://github.com/golemcloud/golem/releases

## Available Libraries

From your component (or shared workspace) `Cargo.toml`:
- `golem-rust` (with `export_golem_agentic` feature) — agent framework, durability, transactions
- `wstd` — WASI standard library (HTTP client via `wstd::http`, async I/O, etc.)
- `log` — logging (uses `wasi-logger` backend, logs visible via `golem agent stream`)
- `serde` / `serde_json` — serialization
- Optional: `golem-wasi-http` — advanced HTTP client alternative

To enable AI features, add the relevant golem-ai provider crate as a dependency (e.g., `golem-ai-llm-openai`). 

## Key Constraints

- Target is `wasm32-wasip2` — no native system calls, threads, or platform-specific code
- Crate type must be `cdylib` for component crates
- All agent method parameters passed by value (no references)
- All custom types need `Schema` derive (plus `IntoValue` and `FromValueAndType`, which `Schema` implies)
- `proc-macro-enable` must be true in rust-analyzer settings (already configured in `.vscode/settings.json`)
- `golem-temp/` and `target/` are gitignored build artifacts, do not manually edit files in those directories

## Formatting and Linting

```shell
cargo fmt                            # Format code
cargo clippy --target wasm32-wasip2  # Lint (must target wasm32-wasip2)
```

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

- App manifest reference: https://learn.golem.cloud/app-manifest
- Full docs: https://learn.golem.cloud
- golem-rust SDK: https://docs.rs/golem-rust
<!-- golem-managed:guide:rust:end -->

