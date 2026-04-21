<!-- golem-managed:guide:ts:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Skills

This project includes coding-agent skills in `.agents/skills/`. Load a skill when the task matches its description.

| Skill | Description |
|-------|-------------|
| `golem-new-project` | Creating a new Golem application project with `golem new` |
| `golem-add-component` | Adding a new component or agent templates to an existing application |
| `golem-edit-manifest` | Editing the Golem Application Manifest (golem.yaml) — components, agents, templates, environments, httpApi, mcp, bridge SDKs, plugins, and more |
| `golem-build` | Building a Golem application with `golem build` |
| `golem-deploy` | Deploying a Golem application with `golem deploy` |
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
| `golem-add-config-ts` | Adding typed configuration to a TypeScript Golem agent |
| `golem-add-secret-ts` | Adding secrets to TypeScript Golem agents |
| `golem-profiles-and-environments` | Understanding CLI profiles, app environments, and component presets — switching between local/cloud, managing deployment targets, and activating per-environment configuration |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-file-io-ts` | Reading and writing files from agent code |
| `golem-js-runtime` | JavaScript runtime environment: available Web APIs, Node.js modules, and npm compatibility |
| `golem-add-llm-ts` | Adding LLM and AI capabilities using third-party npm libraries |
| `golem-make-http-request-ts` | Making outgoing HTTP requests from agent code using fetch |
| `golem-view-agent-logs` | Viewing agent logs and output via streaming |
| `golem-view-agent-files` | Listing files in an agent's virtual filesystem |
| `golem-list-and-filter-agents` | Listing and querying agents with filters |
| `golem-get-agent-metadata` | Checking agent metadata and status |
| `golem-debug-agent-history` | Querying the operation log |
| `golem-undo-agent-state` | Reverting agent state by undoing operations |
| `golem-interrupt-resume-agent` | Interrupting and resuming a Golem agent |
| `golem-test-crash-recovery` | Simulating a crash on an agent for testing crash recovery |
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

## Building

```shell
npm install                      # Install dependencies (run once, or after adding packages)
golem build                      # Build all components
golem component build my:comp    # Build a specific component
```

The build runs TypeScript type-checking, type metadata generation via `golem-typegen`, Rollup bundling, QuickJS WASM injection, agent wrapper generation, and WASM composition. Output goes to `golem-temp/`.

Do NOT run `npx rollup` or `npx tsc` directly — always use `golem build` which orchestrates the full pipeline including type metadata generation and WASM component linking.

## Deploying and Running

```shell
golem server run                 # Start local Golem server
golem deploy                     # Deploy all components to the configured server
golem deploy --try-update-agents # Deploy and update running agents
golem deploy --reset             # Deploy and delete all previously created agents
```

**WARNING**: `golem server run --clean` deletes all existing state (agents, data, deployed components). Never run it without explicitly asking the user for confirmation first.

After starting the server, components must be deployed with `golem deploy` before agents can be invoked. When iterating on code changes, use `golem deploy --reset` to delete all previously created agents — without this, existing agent instances continue running with the old component version. This is by design: Golem updates do not break existing running instances.

To try out agents after deploying, load the `golem-invoke-agent-ts` skill for invoking agent methods from the CLI, or write a script and run it with `golem repl` for interactive testing. The Golem server must be running in a separate process before invoking or testing agents.

## Testing Agents with the REPL

```shell
golem repl                       # Interactive scripting REPL
```

## Defining Agents

Load the `golem-add-agent-ts` skill for defining agents and custom types. See also the skill table above for durability configuration, annotations, RPC, atomic blocks, and transactions.

## Application Manifest (golem.yaml)

- Root `golem.yaml`: app name, includes, environments, and `components` entries
- `golem-temp/common/ts/golem.yaml`: generated on-demand build templates (TypeScript compilation, Rollup bundling, WASM composition) shared by all TS components

Key fields in each `components.<name>` entry:
- `dir`: component directory (`"."` for single-component apps)
- `templates`: references a template from common golem.yaml (e.g., `ts`)
- `env`: environment variables passed to agents at runtime
- `dependencies`: WASM dependencies (e.g., LLM providers from golem-ai)

## Available Libraries

From root `package.json`:
- `@golemcloud/golem-ts-sdk` — agent framework, durability, transactions, RPC
- `@golemcloud/golem-ts-typegen` (dev) — type metadata generation for the build pipeline
- `rollup` with plugins (dev) — bundling TypeScript to JS for WASM injection
- `typescript` (dev) — TypeScript compiler

To enable AI features, uncomment the relevant provider dependency in the component's `golem.yaml` and set the corresponding environment variables.

## Debugging

Load the `golem-get-agent-metadata` skill for checking agent state. Load the `golem-view-agent-logs` skill for streaming agent stdout, stderr, and log channels. Load the `golem-debug-agent-history` skill for querying the operation log. Load the `golem-undo-agent-state` skill for reverting invocations. To invoke agent methods, load the `golem-invoke-agent-ts` skill.

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

## Documentation

- App manifest reference: https://learn.golem.cloud/app-manifest
- Name mapping: https://learn.golem.cloud/name-mapping
- Type mapping: https://learn.golem.cloud/type-mapping
- Full docs: https://learn.golem.cloud

<!-- golem-managed:guide:ts:end -->
