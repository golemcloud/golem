<!-- golem-managed:guide:ts:start -->
<!-- Golem manages this section. Do not edit manually. -->

# Skills

This project includes coding-agent skills in `.agents/skills/`. Load a skill when the task matches its description.

| Skill | Description |
|-------|-------------|
| `golem-new-project` | Creating a new Golem application project with `golem new` |
| `golem-build` | Building a Golem application with `golem build` |
| `golem-deploy` | Deploying a Golem application with `golem deploy` |
| `golem-add-npm-package` | Adding an npm package dependency to the project |
| `golem-add-agent-ts` | Adding a new agent type to a TypeScript Golem component |
| `golem-configure-durability-ts` | Choosing between durable and ephemeral agents |
| `golem-annotate-agent-ts` | Adding prompt and description annotations to agent methods |
| `golem-call-another-agent-ts` | Calling another agent and awaiting the result (RPC) |
| `golem-fire-and-forget-ts` | Triggering an agent invocation without waiting for the result |
| `golem-schedule-future-call-ts` | Scheduling a future agent invocation |
| `golem-atomic-block-ts` | Atomic blocks, persistence control, and idempotency |
| `golem-add-transactions-ts` | Saga-pattern transactions with compensation |
| `golem-add-http-endpoint-ts` | Exposing an agent over HTTP with mount paths and endpoint decorators |
| `golem-http-params-ts` | Mapping path, query, header, and body parameters for HTTP endpoints |
| `golem-add-http-auth-ts` | Enabling authentication and receiving Principal on HTTP endpoints |
| `golem-add-cors-ts` | Configuring CORS allowed origins for HTTP endpoints |
| `golem-configure-api-domain` | Configuring HTTP API domain deployments and security schemes in golem.yaml |
| `golem-make-http-request-ts` | Making outgoing HTTP requests from agent code using fetch |

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

To try out agents after deploying, use `golem agent invoke` for individual method calls, or write a Rib script and run it with `golem repl` for interactive testing. The Golem server must be running in a separate process before invoking or testing agents.

## Name Mapping (Kebab-Case Convention)

All TypeScript identifiers are converted to **kebab-case** when used externally (in CLI commands, Rib scripts, REPL, agent IDs, and WAVE values). This applies to:

- **Agent type names**: `CounterAgent` → `counter-agent`
- **Method names**: `getCount` → `get-count`, `increment` → `increment`
- **Record/object field names**: `fieldName` → `field-name`
- **Variant/union tag names**: `myCase` → `my-case`

This conversion is automatic and consistent across all external interfaces.

## Testing Agents

### Using the REPL

```shell
golem repl                       # Interactive Rib scripting REPL
```

In the REPL, use kebab-case names and WAVE-encoded values:
```rib
let agent = counter-agent("my-counter")
agent.increment()
agent.increment()
```

### Using `golem agent invoke`

Invoke agent methods directly from the CLI. The method name must be fully qualified:

```shell
# Method name format: <component-name>/<agent-type>.{method-name}
# All names in kebab-case

golem agent invoke 'counter-agent("my-counter")' \
  'my:example/counter-agent.{increment}'

# With arguments (WAVE-encoded)
golem agent invoke 'my-agent("id")' \
  'my:example/my-agent.{set-value}' '"hello world"'

# With a record argument
golem agent invoke 'my-agent("id")' \
  'my:example/my-agent.{update}' '{field-name: "value", count: 42.0}'

# Fire-and-forget (enqueue without waiting for result)
golem agent invoke --enqueue 'counter-agent("c1")' \
  'my:example/counter-agent.{increment}'

# With idempotency key
golem agent invoke --idempotency-key 'unique-key-123' \
  'counter-agent("c1")' 'my:example/counter-agent.{increment}'
```

## WAVE Value Encoding

All argument values passed to `golem agent invoke` and used in Rib scripts follow the [WAVE (WebAssembly Value Encoding)](https://github.com/bytecodealliance/wasm-tools/tree/main/crates/wasm-wave) format. See the full [type mapping reference](https://learn.golem.cloud/type-mapping).

### TypeScript Type to WAVE Mapping

| TypeScript Type | WIT Type | WAVE Example |
|-----------------|----------|--------------|
| `string` | `string` | `"hello world"` |
| `boolean` | `bool` | `true`, `false` |
| `number` | `f64` | `1234.0` |
| `Array<T>` | `list<T>` | `[1.0, 2.0, 3.0]` |
| `Map<K, V>` | `list<tuple<K, V>>` | `[("key1", 100.0), ("key2", 200.0)]` |
| `T \| undefined` or `T \| null` | `option<T>` | `some("value")`, `none` |
| `object` / interface | `record { ... }` | `{field-name: "value", count: 42.0}` |
| `{ tag: "x", val: T }` union | `variant { ... }` | `my-case("data")` |
| `"x" \| "y"` string literal union | `enum { ... }` | `my-variant` |
| tuple | `tuple<...>` | `("hello", 1234.0, true)` |
| `Uint8Array` | `list<u8>` | `[104, 101, 108]` |

### WAVE Encoding Rules

**Strings**: double-quoted with escape sequences (`\"`, `\\`, `\n`, `\t`, `\r`, `\u{...}`)
```
"hello \"world\""
```

**Records**: field names in kebab-case, optional fields (`T | undefined`) can be omitted (defaults to `none`)
```
{required-field: "value", optional-field: some(42.0)}
{required-field: "value"}
```

**Variants**: case name in kebab-case, with optional payload in parentheses
```
my-case
my-case("payload")
```

**Options**: can use shorthand (bare value = `some`)
```
some(42.0)    // explicit
42.0          // shorthand for some(42.0), only for non-option/non-result inner types
none
```

**Results**: can use shorthand (bare value = `ok`)
```
ok("value")   // explicit ok
err("oops")   // explicit err
"value"       // shorthand for ok("value")
```

**Flags**: set of labels in curly braces
```
{read, write}
{}
```

**Keywords as identifiers**: prefix with `%` if a name conflicts with `true`, `false`, `some`, `none`, `ok`, `err`, `inf`, `nan`
```
%true
%none
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

```shell
golem agent get '<agent-id>'                    # Check agent state
golem agent stream '<agent-id>'                 # Stream live logs
golem agent oplog '<agent-id>'                  # View operation log
golem agent revert '<agent-id>' --number-of-invocations 1  # Revert last invocation
golem agent invoke '<agent-id>' 'method' args   # Invoke method directly
```

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
