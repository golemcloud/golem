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
| `golem-edit-manifest` | Editing the Golem Application Manifest (golem.yaml) |
| `golem-build` | Building a Golem application with `golem build` |
| `golem-troubleshoot-build` | Troubleshooting Golem build failures and manifest (golem.yaml) configuration |
| `golem-deploy` | Deploying a Golem application with `golem deploy` |
| `golem-local-dev-server` | Starting, configuring, and debugging the local Golem development server with `golem server` |
| `golem-rollback` | Rolling back a Golem deployment to a previous revision or version |
| `golem-redeploy-agents` | Redeploying existing agents by deleting and recreating them |
| `golem-add-agent-ts` | Adding a new TypeScript agent type with `defineAgent` / `.implement` |
| `golem-add-npm-package` | Adding an npm package dependency to a TypeScript Golem project |
| `golem-configure-durability-ts` | Choosing between durable and ephemeral agents |
| `golem-stateless-agent-ts` | Creating ephemeral (stateless) agents with a fresh instance per invocation |
| `golem-annotate-agent-ts` | Adding `description` / `promptHint` annotations to agents and methods |
| `golem-mark-read-only-ts` | Marking methods `readOnly` for a side-effect-free guarantee and result caching |
| `golem-add-config-ts` | Adding typed configuration to a TypeScript agent |
| `golem-add-secret-ts` | Adding secrets (`s.secret`, `Secret<T>`) to TypeScript agents |
| `golem-call-another-agent-ts` | Calling another agent and awaiting the result (RPC) with `clientFor` |
| `golem-call-from-external-ts` | Calling agents from external Node.js apps using generated bridge SDKs |
| `golem-fire-and-forget-ts` | Triggering an agent invocation without waiting for the result (`.trigger`) |
| `golem-parallel-workers-ts` | Fan out work to multiple parallel agents and collect results |
| `golem-schedule-future-call-ts` | Scheduling a future agent invocation (`.schedule`) |
| `golem-recurring-task-ts` | Recurring (cron-like) tasks via self-scheduling |
| `golem-wait-for-external-input-ts` | Waiting for external input using Golem promises (human-in-the-loop) |
| `golem-add-webhook-ts` | Creating and awaiting webhooks for webhook-driven external APIs |
| `golem-multi-instance-agent-ts` | Creating multiple agent instances with phantom agents |
| `golem-atomic-block-ts` | Atomic blocks, persistence control, and idempotency |
| `golem-add-transactions-ts` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-ts` | Exposing an agent over HTTP with mount paths and endpoints |
| `golem-http-params-ts` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-ts` | Enabling authentication on HTTP endpoints |
| `golem-add-cors-ts` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-configure-mcp-server` | Configuring MCP (Model Context Protocol) server deployments in golem.yaml |
| `golem-manage-plugins` | Managing Golem plugins via golem.yaml or CLI |
| `golem-custom-snapshot-ts` | Enabling snapshotting and custom snapshot save/load functions |
| `golem-retry-policies-ts` | Configuring semantic retry policies |
| `golem-quota-ts` | Adding resource quotas (rate limiting, capacity, concurrency) |
| `golem-add-postgres-ts` | Connecting to PostgreSQL with `golem:rdbms/postgres` |
| `golem-add-mysql-ts` | Connecting to MySQL with `golem:rdbms/mysql` |
| `golem-add-ignite-ts` | Connecting to Apache Ignite 2 with `golem:rdbms/ignite2` |
| `golem-add-llm-ts` | Adding LLM and AI capabilities using golem-ai libraries |
| `golem-make-http-request-ts` | Making outgoing HTTP requests from agent code with `fetch` |
| `golem-file-io-ts` | Reading and writing files from agent code |
| `golem-logging-ts` | Adding logging to a TypeScript agent (`console.log` / `wasi:logging`) |
| `golem-enable-otlp-ts` | Enabling the OpenTelemetry (OTLP) plugin for a TypeScript agent |
| `golem-profiles-and-environments` | CLI profiles, app environments, and component presets |
| `golem-add-env-vars` | Defining environment variables for agents in golem.yaml and via CLI |
| `golem-add-initial-files` | Adding initial files to agent filesystems via golem.yaml |
| `golem-create-agent-instance-ts` | Creating a new agent instance with `golem agent new` |
| `golem-invoke-agent-ts` | Invoking a Golem agent method from the CLI |
| `golem-trigger-agent-ts` | Triggering a fire-and-forget invocation from the CLI |
| `golem-schedule-agent-ts` | Scheduling a future invocation from the CLI |
| `golem-interactive-repl-ts` | Using the Golem REPL for interactive testing and scripting |
| `golem-view-agent-logs` | Viewing agent logs and output via streaming |
| `golem-view-agent-files` | Listing files in an agent's virtual filesystem |
| `golem-list-and-filter-agents` | Listing and querying agents with filters |
| `golem-get-agent-metadata` | Checking agent metadata and status |
| `golem-debug-agent-history` | Querying the operation log |
| `golem-undo-agent-state` | Reverting agent state by undoing operations |
| `golem-interrupt-resume-agent` | Interrupting and resuming a Golem agent |
| `golem-test-crash-recovery` | Simulating a crash on an agent for testing crash recovery |
| `golem-integration-test-setup` | Setting up a dedicated Golem environment for integration testing |
| `golem-cancel-queued-invocation` | Canceling a pending (queued) invocation on an agent |
| `golem-delete-agent` | Deleting an agent instance |

# Golem Application Development Guide (TypeScript)

## Overview

This is a **Golem Application** — a distributed computing project. TypeScript components are bundled and executed on the Golem platform, which provides durable execution, persistent state, and agent-to-agent communication.

Key concepts:
- **Component**: A deployable unit built from your TypeScript sources, defining one or more agent types
- **Agent type**: Declared with `defineAgent({ ... })` and given behaviour with `.implement({ ... })` from `@golemcloud/golem-ts-sdk`
- **Agent (worker)**: A running instance of an agent type, identified by its `id` record values, with persistent state

The SDK is the **fluent (Standard Schema)** API: method inputs and return values are described with [Standard Schema](https://standardschema.dev/) values. **Zod** is used throughout these examples; **Valibot** and **ArkType** also work (any Standard Schema vendor). There are no classes and no decorators.

## Agent Fundamentals

- Every agent is uniquely identified by its **`id` record values** — two agents with the same id values are the same agent. The id fields are the constructor parameters.
- Agents are **durable by default** — state persists across invocations, failures, and restarts.
- Invocations are processed **sequentially in a single thread** — no concurrency within a single agent, no locks needed.
- Agents can **spawn / call other agents** via **RPC** (see Calling Other Agents).
- An agent is created implicitly on first invocation — no separate creation step needed.
- **Futures cannot outlive invocations** — every `Promise` started during an invocation must be `await`ed before the handler returns; do not store unresolved promises in state to poll them from a later invocation.

## Durability & Automatic Retries

Golem **automatically retries** failed operations using durable execution. **Do not add manual retry loops, `while` retry patterns, or backoff utilities in agent code** — let operations fail and Golem will retry them. A built-in default policy (3 retries, exponential backoff with jitter) applies when no user-defined policy matches.

Retried transparently: outgoing HTTP requests (`fetch`), RPC calls between agents, database / storage calls (`golem:rdbms/*`, keyvalue, blobstore), and thrown errors at the top of a handler (the worker is restarted and the invocation is replayed from the oplog, previously-recorded side effects skipped).

Only customize when the *strategy* needs to change — see `golem-retry-policies-ts`.

## Project Structure

```
golem.yaml                     # Golem Application Manifest
package.json                   # npm dependencies (zod, etc.)
tsconfig.json                  # moduleResolution: "bundler"; NO experimentalDecorators
src/
  main.ts                      # Entrypoint: imports each agent module for its side effects
  <agent-name>.ts              # Agent definition (defineAgent) + implementation (.implement)
golem-temp/                    # Build artifacts (gitignored)
```

`src/main.ts` must import every agent module for side effects (`import './counter-agent.js';`). `defineAgent` / `.implement` register the agent at module-load time, so importing the module is enough for discovery — nothing needs to be exported for the runtime.

## Prerequisites

- Node.js
- Golem CLI (`golem`): download from https://github.com/golemcloud/golem/releases

## Defining an Agent

`defineAgent(...)` declares the contract; `.implement(...)` supplies handlers whose `this` is bound to the state returned by `init`.

```typescript
import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

export const Counter = defineAgent({
  name: 'Counter',                  // wire-level agent type name
  id: { name: z.string() },         // identity record → constructor parameters
  methods: {
    value: method({ input: {}, returns: z.number() }),
    add: method({ input: { by: z.number() }, returns: z.number() }),
    reset: method({ input: {}, returns: z.void() }),
  },
});

export const CounterImpl = Counter.implement({
  init: () => ({ count: 0 }),       // returns the initial state; `this` is bound to it
  methods: {
    value() { return this.count; },
    add({ by }) { this.count += by; return this.count; },
    reset() { this.count = 0; },
  },
});
```

## Methods

`method({ input, returns, readOnly?, description?, promptHint?, http? })`:

- `input` is a record of one Standard Schema per named parameter; the handler receives them as a single destructured object. An empty `input: {}` means a no-argument handler.
- `returns` is the success-value schema; use `z.void()` for no return value.
- `readOnly: true` marks a side-effect-free method (result caching, HTTP cache headers). It is **boolean only**.
- `description` / `promptHint` add discovery metadata for AI/LLM tooling.

## Schemas & the `s` markers

Standard Schema covers ordinary shapes (`z.string()`, `z.number()` = f64, `z.boolean()`, `z.object({...})`, `z.array(...)`, `z.enum([...])`). **TypeScript enums are not supported — use `z.enum([...])`.**

For WIT types Standard Schema cannot express on its own, import the vendor-neutral marker namespace `s`:

```typescript
import { s } from '@golemcloud/golem-ts-sdk';

s.u8() s.u16() s.u32() s.u64()   // sized integers (u64/s64/durations use bigint)
s.s8() s.s16() s.s32() s.s64() s.f32()
s.char() s.datetime() s.duration() s.url() s.bytes()   // s.bytes() ↔ Uint8Array
s.int32Array() s.float64Array() /* …other typed arrays… */
s.secret(z.string())             // a secret config field (see Config & Secrets)
s.result(okSchema, errSchema)    // a typed result<ok, err> return (see Typed Errors)
```

## State

State lives entirely in the object returned by `init()` and is read/written through `this` in the handlers — there are no class fields. `init` receives an `InitContext` (`{ id, principal, phantomId, config }`); handlers' `this` also carries SDK helpers `getId()`, `getPhantomId()`, `getPrincipal()`, and `config`.

```typescript
export const HttpAgentImpl = HttpAgent.implement({
  init: ({ id }) => ({ name: id.name }),
  methods: { hello({ who }) { return `Hello, ${who}! (from ${this.name})`; } },
});
```

## Typed Errors

Return a WIT `result<ok, err>` by setting `returns: s.result(ok, err)`, and return `Result.ok(...)` / `Result.err(...)`. The failure travels as a value inside the success payload (the caller receives a decoded `Result`).

```typescript
import { defineAgent, method, s, Result } from '@golemcloud/golem-ts-sdk';

divide: method({ input: { a: z.number(), b: z.number() }, returns: s.result(z.number(), z.string()) }),
// handler:
divide({ a, b }) { return b === 0 ? Result.err('div by zero') : Result.ok(a / b); }
```

## HTTP

Declare an HTTP surface with `http.mount(...)` on `defineAgent` and per-method `http` endpoints. Mount `{var}` names bind to `id` fields; endpoint `{var}` names bind to method inputs — both are checked at compile time (template-literal typed).

```typescript
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';

export const TaskAgent = defineAgent({
  name: 'TaskAgent',
  id: { name: z.string() },
  http: http.mount('/task-agents/{name}', { cors: ['*'] }),   // also: { auth: true }
  methods: {
    createTask: method({ input: { title: z.string() }, returns: Task, http: http.post('/tasks') }),
    getTasks:   method({ input: {}, returns: z.array(Task), http: http.get('/tasks') }),
    complete:   method({ input: { id: z.number() }, returns: Task.nullable(), http: http.post('/tasks/{id}/complete') }),
  },
});
```

Query binding uses the inline `?key={var}` template (e.g. `http.get('/hello?who={who}')`); header binding uses `{ headers: { 'X-Name': 'who' } }`. Verbs: `http.get/head/post/put/del/patch/options/connect/trace` and `http.custom(verb, path)`.

## Config & Secrets

Declare a single `config` record on `defineAgent`. Any field (at any depth) wrapped in `s.secret(inner)` is a secret; every other field is a plain local value. `this.config` is statically typed: local fields read their value fresh on each access, secret fields yield a lazy `Secret<T>` handle — call `.get()` to reveal the plaintext. **Never log a secret; `.get()` fresh at the point of use.**

```typescript
export const ConfigAgent = defineAgent({
  name: 'ConfigAgent',
  id: { name: z.string() },
  config: {
    greeting: z.string(),               // local → string
    apiKey: s.secret(z.string()),       // secret → Secret<string>
  },
  methods: {
    greet:   method({ input: { who: z.string() }, returns: z.string() }),
    keyTail: method({ input: {}, returns: z.string() }),
  },
});

export const ConfigAgentImpl = ConfigAgent.implement({
  init: () => ({}),
  methods: {
    greet({ who }) { return `${this.config.greeting}, ${who}!`; },
    keyTail()      { return this.config.apiKey.get().slice(-4); },
  },
});
```

Config values are provisioned via `golem.yaml` (`env`/`envDefaults`/`secretDefaults`) and the CLI. See `golem-add-config-ts` and `golem-add-secret-ts`.

## Calling Other Agents (RPC)

`clientFor(Def)` returns a factory; call it with an id record to get a typed proxy. `await client.m(input)` invokes and awaits; `client.m.trigger(input)` is fire-and-forget; `client.m.schedule(at, input)` enqueues for later.

```typescript
import { clientFor } from '@golemcloud/golem-ts-sdk';
import { Counter } from './counter-agent.js';

const counter = clientFor(Counter);
const next = await counter({ name: 'c1' }).add({ by: 5 });
counter({ name: 'c1' }).add.trigger({ by: 1 });   // fire-and-forget
```

## Snapshotting

Opt in with the `snapshotting` option on `defineAgent`. Give it `{ policy, state }` where `state` is a schema so **only the schema-declared fields** of `this` are serialized (typed + scoped):

```typescript
snapshotting: { state: z.object({ count: z.number() }), policy: { everyNInvocations: 5 } },
```

Policy is `'disabled'` (default) | `'default'` | `{ everyNInvocations: n }` | `{ periodicSeconds: n }`. A bare policy without `state` falls back to reflective JSON serialization of the whole state. For full control over the bytes, supply a `snapshot: { save, load }` block on `.implement(...)` (`save` returns `Uint8Array`, `load` restores from it). See `golem-custom-snapshot-ts`.

## Durability Primitives

Host helpers, importable from `@golemcloud/golem-ts-sdk`:

- `atomically(fn)` — run a region that commits on success and rolls back + retries on a thrown error.
- `checkpoint()` — `cp.runOrRevert(() => Result.ok/err(...))` returns the ok value or reverts (uncatchable) the invocation.
- `durable(spec, request, body)` — run a non-deterministic side effect once, persist its typed result, and replay (not re-run) it on recovery. Uses `FunctionType` for the commit/replay policy.
- Sagas — `compensable(run, compensate)` steps composed with `fallibleSaga(...)` / `infallibleSaga(...)` for transactional compensation.
- Promises (human-in-the-loop) — `createPromise()`, `await awaitPromise(id)`, `completePromise(id, bytes)`.

## Available Libraries

- `@golemcloud/golem-ts-sdk` — the agent framework, schema markers, host helpers, typed keyvalue / blobstore / rdbms / websocket surfaces.
- `zod` (default), or `valibot` / `arktype` — Standard Schema vendors.
- Node built-ins where supported (e.g. `node:sqlite`'s `DatabaseSync` for an embedded DB).
- `fetch` for outgoing HTTP; `console.log` for logging (visible via `golem agent stream`).

## Key Constraints

- The fluent SDK has **no classes and no decorators**. `tsconfig.json` uses `"moduleResolution": "bundler"` and must NOT set `experimentalDecorators` / `emitDecoratorMetadata`.
- `z.number()` maps to WIT `f64`; use the `s.*` integer markers for sized ints, and `bigint` for 64-bit values.
- TypeScript enums are unsupported — use `z.enum([...])`.
- Every agent module must be imported from `src/main.ts`.
- `golem-temp/` is a gitignored build artifact — do not edit files there.

## Build, Deploy, and Invoke

```shell
golem build                                  # Bundle + build the component(s)
golem deploy                                 # Build and deploy
golem agent invoke <agent-id> <method> ...   # Invoke a method (see golem-invoke-agent-ts)
golem agent stream <agent-id>                # Stream an agent's logs
```

## Running Golem CLI commands non-interactively

The `golem` CLI prompts for confirmation before mutating changes. In non-interactive contexts (CI, scripts, coding agents) **always pass `--yes` (or `-y`)** to mutating commands:

```shell
golem build --yes
golem deploy --yes
golem new --yes --template ts <APPLICATION_PATH>
golem agent update --yes <AGENT>
```

If you see `This action requires confirmation, but the current shell is non-interactive.` followed by `Failed to build application`, re-run the same command with `--yes`.

## Documentation

- App manifest reference: https://learn.golem.cloud/app-manifest
- Full docs: https://learn.golem.cloud
- Standard Schema: https://standardschema.dev

<!-- golem-managed:guide:ts:end -->
