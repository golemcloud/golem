<!-- golem-managed:guide:ts:start -->
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
| `golem-create-agent-instance-ts` | Creating a new agent instance with `golem agent new` |
| `golem-invoke-agent-ts` | Invoking a Golem agent method from the CLI |
| `golem-trigger-agent-ts` | Triggering a fire-and-forget invocation on a Golem agent |
| `golem-schedule-agent-ts` | Scheduling a future invocation on a Golem agent |
| `golem-add-npm-package` | Adding an npm package dependency to the project |
| `golem-add-postgres-ts` | Connecting to PostgreSQL with `golem:rdbms/postgres` from TypeScript agents |
| `golem-add-mysql-ts` | Connecting to MySQL with `golem:rdbms/mysql` from TypeScript agents |
| `golem-add-ignite-ts` | Connecting to Apache Ignite 2 with `golem:rdbms/ignite2` from TypeScript agents |
| `golem-add-agent-ts` | Adding a new agent type to a TypeScript Golem component |
| `golem-configure-durability-ts` | Choosing between durable and ephemeral agents |
| `golem-stateless-agent-ts` | Creating ephemeral (stateless) agents with a fresh instance per invocation |
| `golem-annotate-agent-ts` | Adding prompt and description annotations to agent methods |
| `golem-call-another-agent-ts` | Calling another agent and awaiting the result (RPC) |
| `golem-call-from-external-ts` | Calling agents from external TypeScript/Node.js applications using generated bridge SDKs |
| `golem-fire-and-forget-ts` | Triggering an agent invocation without waiting for the result |
| `golem-parallel-workers-ts` | Fan out work to multiple parallel agents and collect results |
| `golem-schedule-future-call-ts` | Scheduling a future agent invocation |
| `golem-recurring-task-ts` | Implementing recurring (cron-like) tasks via self-scheduling — periodic polling, cleanup, heartbeats, backoff, and cancellation |
| `golem-wait-for-external-input-ts` | Waiting for external input using Golem promises (human-in-the-loop, webhooks, external events) |
| `golem-add-webhook-ts` | Creating and awaiting webhooks for integrating with webhook-driven external APIs |
| `golem-multi-instance-agent-ts` | Creating multiple agent instances with the same constructor parameters using phantom agents |
| `golem-atomic-block-ts` | Atomic blocks, persistence control, and idempotency |
| `golem-add-transactions-ts` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-ts` | Exposing an agent over HTTP with mount paths and endpoint decorators |
| `golem-http-params-ts` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-ts` | Enabling authentication and receiving Principal on HTTP endpoints |
| `golem-add-cors-ts` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins — listing available plugins, installing and configuring plugins via golem.yaml or CLI, and understanding built-in plugins like the OTLP exporter |
| `golem-add-config-ts` | Adding typed configuration to a TypeScript Golem agent |
| `golem-add-secret-ts` | Adding secrets to TypeScript Golem agents |
| `golem-quota-ts` | Adding resource quotas (rate limiting, capacity, concurrency) to TypeScript Golem agents using QuotaToken and reservations |
| `golem-retry-policies-ts` | Configuring semantic retry policies — composable exponential/periodic/fibonacci backoff, predicates on error properties, scoped overrides with `withRetryPolicy`, and live CLI management |
| `golem-profiles-and-environments` | Understanding CLI profiles, app environments, and component presets — switching between local/cloud, managing deployment targets, and activating per-environment configuration |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-file-io-ts` | Reading and writing files from agent code |
| `golem-js-runtime` | JavaScript runtime environment: available Web APIs, Node.js modules, and npm compatibility |
| `golem-add-llm-ts` | Adding LLM and AI capabilities using third-party npm libraries |
| `golem-make-http-request-ts` | Making outgoing HTTP requests from agent code using fetch |
| `golem-logging-ts` | Adding logging to a TypeScript Golem agent using `console` methods |
| `golem-enable-otlp-ts` | Enabling the OpenTelemetry (OTLP) plugin for a TypeScript agent — exporting traces, logs, and metrics to an OTLP collector, adding custom spans with the invocation context API or `node:diagnostics_channel` |
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
| `golem-interactive-repl-ts` | Using the Golem REPL for interactive testing and scripting of agents |

# Golem Application Development Guide (TypeScript)

## Overview

This is a **Golem Application** — a distributed computing project targeting WebAssembly (WASM). Components are compiled from TypeScript via Rollup and QuickJS into WASM modules executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A WASM module compiled from TypeScript, defining one or more agent types
- **Agent type**: A class decorated with `@agent()` extending `BaseAgent`, defining the agent's API
- **Agent (worker)**: A running instance of an agent type, identified by constructor parameters, with persistent state

## Agent Fundamentals

- Every agent is uniquely identified by its **constructor parameter values** — two agents with the same parameters are the same agent
- Agents are **durable by default** — their state persists across invocations, failures, and restarts
- Invocations are processed **sequentially in a single thread** — no concurrency within a single agent, no need for locks
- Agents can **spawn other agents** and communicate with them via **RPC** (see Agent-to-Agent Communication)
- An agent is created implicitly on first invocation — no separate creation step needed
- **Promises cannot outlive invocations** — every `Promise` created during an invocation must settle (be `await`ed or `.then`-chained to completion) before the invocation returns; do not store unresolved promises in agent state to `await` them from a later invocation

## Durability & Automatic Retries

Golem **automatically retries** failed operations using durable execution. **Do not add manual retry loops, `try/catch` + retry patterns, or backoff utilities in agent code** — let operations fail and Golem will retry them. A built-in default policy (3 retries, exponential backoff with jitter, clamped to [100ms, 1s]) applies when no user-defined policy matches.

The following are retried transparently:

- **HTTP requests** to external services (via `fetch`, `node:http`, `node:https`, etc.)
- **RPC calls** between agents
- **Database / storage calls** — `golem:rdbms/postgres`, `golem:rdbms/mysql`, `golem:rdbms/ignite2`, `wasi:blobstore`, `wasi:keyvalue`
- **Uncaught exceptions** (thrown errors / unhandled promise rejections) escaping an agent method — the worker is restarted and the invocation is replayed from the oplog, with all previously-recorded side effects skipped

Only customize when the *strategy* needs to change (different backoff, give-up conditions, per-status-code policies). For that, see the `golem-retry-policies-ts` skill.

## Project Structure

```
# Single-component app
golem.yaml                            # Golem Application Manifest (contains components.<name>.dir = ".")
package.json                          # Root npm dependencies
tsconfig.json                         # Component TypeScript config
src/
  main.ts                             # Module entry point; imports agent modules
  <agent_name>.ts                     # Agent definitions and implementations

# Multi-component app
golem.yaml                            # Golem Application Manifest (components map with explicit dir per component)
package.json                          # NPM dependencies (shared for all components)
<component-a>/
  tsconfig.json                       # Component TypeScript config
  src/
    main.ts                           # Module entry point; imports agent modules
    <agent_name>.ts                   # Agent definitions and implementations
<component-b>/
  tsconfig.json                       # Component TypeScript config
  src/
    main.ts                           # Module entry point; imports agent modules
    <agent_name>.ts                   # Agent definitions and implementations

golem-temp/                           # Build artifacts (gitignored)
  common/                             # Shared Golem templates and configuration (generated on-demand)
    ts/                               # Shared TypeScript Golem templates and configuration
      golem.yaml                      # Build templates for all TS components
      rollup.config.component.mjs     # Shared Rollup configuration
```

`src/main.ts` is an entrypoint module that should import each agent module for side effects (for example, `import './counter-agent';`). Agent classes do not need to be exported for discovery (export them only when another module needs to import them). Importing a module executes it, so if that module contains multiple `@agent()` classes, all of them are discovered.

## Prerequisites

- Node.js (with npm)
- Golem CLI (`golem`): download from https://github.com/golemcloud/golem/releases

## Available Libraries

From root `package.json`:
- `@golemcloud/golem-ts-sdk` — agent framework, durability, transactions, RPC
- `@golemcloud/golem-ts-typegen` (dev) — type metadata generation for the build pipeline
- `rollup` with plugins (dev) — bundling TypeScript to JS for WASM injection
- `typescript` (dev) — TypeScript compiler

To enable AI features, uncomment the relevant provider dependency in the component's `golem.yaml` and set the corresponding environment variables.

## Key Constraints

- Target is WebAssembly via [QuickJS](https://github.com/DelSkayn/rquickjs/) — supports ES2020 including modules, async/await, async generators, Proxies, BigInt, WeakRef, FinalizationRegistry, and all standard built-ins (Array, Map, Set, Promise, RegExp, Date, JSON, Math, typed arrays, etc.)
- Golem's JS runtime implements a broad set of Browser and Node.js APIs (documented in the [wasm-rquickjs README](https://github.com/golemcloud/wasm-rquickjs)). The following are available out of the box:
    - **Web Platform APIs**: `fetch`, `Headers`, `Request`, `Response`, `FormData`, `Blob`, `File`, `URL`, `URLSearchParams`, `console`, `setTimeout`/`clearTimeout`, `setInterval`/`clearInterval`, `setImmediate`, `AbortController`, `AbortSignal`, `DOMException`, `TextEncoder`, `TextDecoder`, `TextEncoderStream`, `TextDecoderStream`, `ReadableStream`, `WritableStream`, `TransformStream`, `structuredClone`, `crypto.randomUUID`, `crypto.getRandomValues`, `Event`, `EventTarget`, `CustomEvent`, `MessageChannel`, `MessagePort`, `Intl` (DateTimeFormat, NumberFormat, Collator, PluralRules)
    - **Node.js modules**: `node:buffer`, `node:crypto` (hashes, HMAC, ciphers, key generation, sign/verify, DH, ECDH, X509, etc.), `node:dgram` (UDP sockets), `node:dns`, `node:events` (EventEmitter), `node:fs` and `node:fs/promises` (comprehensive filesystem API), `node:http`/`node:https` (client and server), `node:module`, `node:net` (TCP sockets and servers), `node:os`, `node:path`, `node:perf_hooks`, `node:process`, `node:punycode`, `node:querystring`, `node:readline`, `node:sqlite` (embedded SQLite, requires feature flag), `node:stream` and `node:stream/promises`, `node:string_decoder`, `node:test`, `node:timers`, `node:url`, `node:util`, `node:v8`, `node:vm`, `node:zlib` (gzip, deflate, brotli)
    - **Stubs** (throw or no-op for compatibility): `node:child_process`, `node:cluster`, `node:http2`, `node:inspector`, `node:tls`, `node:worker_threads`
- Additional npm dependencies can be installed with `npm install` — most packages targeting browsers or using the Node.js APIs listed above will work
- Check the [wasm-rquickjs README](https://github.com/golemcloud/wasm-rquickjs) for the most up-to-date list of available APIs
- TypeScript **enums are not supported** — use string literal unions instead
- All agent classes must extend `BaseAgent` and be decorated with `@agent()`
- Constructor parameters define agent identity — they must be serializable types
- Do not manually edit files in `golem-temp/` — they are auto-generated build artifacts
- The build pipeline uses `golem-typegen` to extract type metadata from TypeScript via decorators; ensure `experimentalDecorators` and `emitDecoratorMetadata` are enabled in `tsconfig.json`

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
- Name mapping: https://learn.golem.cloud/name-mapping
- Type mapping: https://learn.golem.cloud/type-mapping
- Full docs: https://learn.golem.cloud
<!-- golem-managed:guide:ts:end -->

