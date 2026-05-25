---
title: "Golem 1.5: The Agent Runtime"
date: "2026-05-08"
author: "John A. De Goes"
tags: ["Announcements", "Product Updates"]
slug: "golem-1-5-the-agent-runtime"
originalUrl: "https://www.linkedin.com/pulse/golem-15-agent-runtime-john-de-goes-hcrde/"
---

On Thursday, May 14th, we are excited to announce the release of Golem 1.5, alongside a launch event and the kickoff of The Golem Forge Hackathon, with a $2,500 cash prize.

Golem is a durable agent runtime. Open source, WebAssembly-based, designed for long-running stateful agents that survive failures, restarts, redeploys, and time. Not a library you import into your application. The runtime your application runs on.

A framework is a library that lives inside someone else's runtime — a Lambda, a container, a Node.js process — and inherits whatever guarantees that runtime offers. A runtime defines its own execution model. The difference matters because durability, exactly-once execution, transparent recovery, replay, and structural resource control are properties of the execution model itself. They cannot be retrofitted by a library, no matter how clever. Agents that handle real money, real customers, and real consequences need a runtime built for them. That is what Golem is, and it is what every feature in this release is designed to make more true.

Golem 1.5 makes Golem agent-first across every surface — HTTP, MCP, configuration, deployment, observability, retry, resource control, and tooling. There are seventeen new capabilities in this release, and they fall into the following categories:

## Scala

Golem 1.5 ships first-class Scala support via Scala.js compiled into Golem's QuickJS-based JavaScript runtime. The Scala SDK provides code-first agent definitions through annotations, generated RPC client classes for typesafe agent-to-agent communication, sbt-plugin integration with `golem build`, and zio-http compatibility for HTTP requests. The compilation chain — Scala to JavaScript to WASM — is unusual, but it works, and it allowed us to ship Scala on Golem now rather than wait for direct Scala-to-WASM toolchains to mature. Two features remain to land in a future release: a native Scala REPL and a Scala bridge generator.

## MoonBit

Golem 1.5 ships first-class MoonBit support, with derive-attribute-based agent definitions, code-first routes, and RPC clients. MoonBit produces very small WASM binaries — meaningfully smaller than the alternatives — which translates directly into faster agent instantiation. The MoonBit SDK is built on a code-level transformation tool that parses the user's source, finds Golem-specific derive attributes, and generates the necessary typeclass implementations and registration code. As with Scala, a native REPL and bridge generator are deferred to a future release; the TypeScript REPL works fully against MoonBit agents.

## JavaScript / TypeScript

The QuickJS-based JavaScript runtime introduced in 1.3 has been validated against Node's own test suite, and compatibility has expanded substantially. About thirty `node:*` modules are now supported alongside the standard Web Platform APIs — `node:fs`, `node:crypto`, `node:http`, `node:stream`, `node:events`, `node:buffer`, `node:url`, `node:path`, `node:os`, `node:dns`, `node:net`, `node:dgram`, `node:zlib`, `node:perf_hooks`, `node:async_hooks`, `node:test`, and many more. A few are explicitly out of scope — `node:child_process`, `node:worker_threads`, `node:http2`, `node:tls`, `node:cluster` — none of which work in our single-threaded sandboxed WASM environment by design. The result is that most third-party JS/TS libraries now work on Golem unmodified, validated by hand on common libraries and by automated coding agent runs across hundreds of others.

## Rust and TypeScript continue

Both continue to be first-class, with the same code-first agent model, code-first routes, RPC, configuration, and tooling as the new languages.

## Code-first HTTP routes

HTTP endpoints are now declared directly on agent methods through decorators or annotations. Mount points use parameter placeholders that map to agent constructor arguments. Endpoint methods take path templates, HTTP methods, header bindings, and CORS configuration. Authentication can be configured per-mount or per-endpoint, and authenticated methods can optionally receive a Principal parameter populated with information about the caller.

Phantom agents — single-line opt-in via `phantomAgent = true` — let stateless gateway agents process requests in fully parallel agent instances rather than serializing through one. OpenAPI specifications are generated automatically from the source, exposed at `openapi.yaml` on every deployment.

## Typed configuration

Configuration is a typed record defined in agent code and injected into the agent's constructor as a `Config<T>`. The configuration type becomes part of the agent's contract, which means deployments verify that all required configuration is satisfied — with the right shape — before anything ships. Default configuration values come from the application manifest, where presets allow shared definitions across multiple agents and component templates apply shared defaults across components.

## Secrets

Secrets are wrapped in a `Secret<T>` type and stored per-environment rather than per-agent deployment. The `get` call fetches the latest stored value each time, which is what supports rotation. Defaults can be set in the manifest with environment-variable substitution. Updates flow through the CLI without redeploying agents.

## Webhooks

Agents can mint awaitable webhook URLs at runtime. While waiting for a webhook to fire, the agent suspends fully — consuming no compute, no memory. When an external system POSTs a payload to the URL, the agent resumes with the payload available as raw bytes, a string, or parsed JSON. This is built on the Promise mechanism Golem has shipped since 1.0; we have now exposed that mechanism as a first-class web primitive.

The application manifest is now thinner than it has ever been. Most of what used to live in YAML now lives in code, where it can be type-checked.

## Model Context Protocol (MCP)

Any Golem agent can be exposed as a Model Context Protocol server through manifest configuration alone — no code required. Agent methods map automatically to MCP tools, resources, or resource templates depending on whether they take parameters and whether the agent is a singleton. OAuth security schemes attach via the CLI, with support for common providers and any custom OAuth2-compatible provider. Method-level `@description` and `@prompt` annotations propagate into MCP metadata. Three special data types — a typed sum over text, binary, and structured data — give MCP a richer surface than most native MCP servers offer. The data types are not MCP-specific; an agent that uses them remains callable through HTTP, RPC, or any other surface.

## Bridge generation

Golem can now generate fully type-safe Rust crates and TypeScript npm packages that let any non-Golem application call Golem agents. Generation is configurable per agent and per language: a manifest can opt in `CounterAgent` for TypeScript and every agent for Rust, for example. Method variants follow the same conventions as agent-to-agent RPC — `get`/`getPhantom`/`newPhantom` constructors, plus `invoke` / trigger-without-await / schedule-for-later variants on every method. Generated bridges work against agents written in any supported language. Bridge generators for Scala and MoonBit are deferred to a future release.

## TypeScript REPL

Rib has been removed from Golem entirely (and offered upstream to wasmtime). The new primary REPL is a TypeScript REPL with all bridge clients pre-loaded in the global scope, plus a large set of built-in commands matching the CLI — build, deploy, log inspection, agent management — all available without leaving the REPL. The TypeScript REPL can call any agent in any supported language.

## WebSockets

Golem agents can now connect to third-party WebSocket servers — the WASI HTTP interface alone could not handle the connection upgrade. Each language gets an idiomatic API: standard browser WebSocket/WebSocketStream in TypeScript and Scala, a tungstenite-inspired API in the Rust SDK, and direct WIT bindings in MoonBit. Durability is partial today: if the server supports transparent reconnection, Golem will reopen the connection on agent restart and continue sending and receiving on the new socket. We expect to improve this further in subsequent releases.

## Retry policies

The old single global retry policy is gone, replaced with a fully composable, predicate-driven system. Policies are defined per-environment in the manifest. Each named policy has a predicate — a boolean expression evaluated against error context properties such as status code, URI scheme, error type, target component, or trap type — and a policy, a tree built from base nodes (`periodic`, `exponential`, `fibonacci`, `immediate`, `never`) and combinators (`countBox`, `timeBox`, `clamp`, `addDelay`, `jitter`, `filteredOn`, `andThen`, `union`, `intersect`). Policies are evaluated in descending priority order, and they can be created, modified, or deleted at runtime via CLI or REST API, with changes affecting running agents immediately.

```yaml
retryPolicyDefaults:
  prod:
    no-retry-4xx:
      priority: 20
      predicate:
        and:
          - propGte: { property: status-code, value: 400 }
          - propLt: { property: status-code, value: 500 }
      policy: "never"
    http-transient:
      priority: 10
      predicate:
        propIn: { property: status-code, values: [502, 503, 504] }
      policy:
        countBox:
          maxRetries: 5
          inner:
            jitter:
              factor: 0.15
              inner:
                clamp:
                  minDelay: "100ms"
                  maxDelay: "5s"
                  inner:
                    exponential:
                      baseDelay: "200ms"
                      factor: 2.0
```

The SDKs expose the same model in code, with scoped overrides via `withRetryPolicy` blocks. A second improvement, orthogonal to the policy system: many transient errors that previously required throwing away the agent instance and replaying its oplog from scratch now retry inline, transparently and quickly. Better classification of which errors are deterministic and which are transient means we no longer retry things known to fail again.

## Snapshots

User-defined snapshotting is now supported, with language-specific defaults. The oplog includes snapshot entries that capture full agent state at recovery checkpoints:

```
#00022:
SNAPSHOT
          at:    2026-04-15T13:19:04.619Z
          data:  {
  "principal": { "tag": "anonymous" },
  "state": { "name": "test1", "value": 5 },
  "version": 1
}
```

Recovery becomes far faster for long-running agents, and debugging gets dramatically easier.

## Per-agent configuration

The agent is now the primary configurable entity. Environment variables, initial filesystem, configuration, secrets, and bridge generation are all configurable per agent. Component templates handle shared defaults; presets within those templates handle per-environment differences. A single component can contain many agents, each with its own typed configuration and its own secrets, deployed together. The result: in 1.5, agent identifiers are just type names plus constructor parameters — component IDs no longer leak into application code.

## Quotas

A new general-purpose distributed resource-tracking system. Resources are defined per-environment with one of three limit types: **Rate** (a refilling pool — useful for API call limits), **Capacity** (a fixed pool that never refills — useful for storage), or **Concurrency** (a fixed pool that returns to the pool on release — useful for connection limits). Each resource has an enforcement action when a token request cannot be satisfied: **reject** (return an error), **throttle** (suspend the agent until the resource is available — fully automatic, no application logic required), or **terminate** (kill the agent). Agents acquire tokens against resources, and tokens can be split across agent-to-agent calls:

```typescript
const childToken: QuotaToken = token.split(200n);
const childAgent = await SummarizerAgent.newPhantom();
const summary = childAgent.summarize(text, childToken);
```

The same primitive handles request rate limiting, LLM token-budget control, concurrent connection management, and any other bounded resource an agent depends on.

All four of these — retries, snapshots, per-agent configuration, quotas — are runtime-level mechanisms, which is why they compose with replay, suspension, and exactly-once execution without further work from the developer.

## Observability

Oplog processor plugins gained exactly-once delivery semantics in 1.5, the missing piece that made them production-ready. The first built-in plugin, `golem-otlp-exporter`, ships with the release and exports traces, logs, and metrics over OTLP/HTTP to any compatible collector. Spans are created automatically for agent invocations, RPC calls, and outgoing HTTP requests. User code can create custom spans through the SDK, and trace and span IDs propagate through inbound and outbound HTTP requests automatically. About two dozen runtime metrics ship by default — invocation counts and durations, fuel consumption, memory growth, oplog processor lag, transaction commit and rollback counts, snapshot sizes, error and interrupt counts, and more — each annotated with the standard `service.name` (the agent type) plus `golem.agent.id`, `golem.component.id`, and `golem.component.version`.

Setup is a few lines in the manifest pointing the exporter at a collector endpoint:

```yaml
plugins:
  - name: golem-otlp-exporter
    version: 1.5.0
    parameters:
      endpoint: "http://localhost:4318"
      signals: "traces,logs,metrics"
```

The Golem repository includes a docker-compose example that brings up an OpenTelemetry Collector with Grafana, Loki, Prometheus, and Jaeger for the three signals.

## Project templates & agent skills

New project templates strip generated build infrastructure down to a handful of files: an AGENTS.md, the manifest, the language's own configuration files, and the source. Build steps that used to be dumped into the project directory are now extracted dynamically by the CLI to a temporary directory — observable for troubleshooting, but not checked into the repository. Upgrading to a new Golem version is now a matter of updating the CLI, not hand-merging build configuration. Multiple templates can be mixed into a single project, and Golem-specific dependencies in `package.json`, `Cargo.toml`, and equivalent files are verified at build time with helpful error messages when they need updating.

Agent skills — instructional artifacts that coding agents read before generating Golem code — ship alongside the templates. About ten to fifteen language-independent skills, plus twenty-five to thirty per language, covering project creation, configuration, agent definition, HTTP and MCP exposure, RPC, databases, external services, AI provider integration, webhooks, transactions, snapshotting, and troubleshooting. We benchmark the skills against popular coding agents weekly.

None of this is the kind of work that goes on a hero slide. All of it is what production deployment actually requires.

## Looking ahead: Golem 1.6

If 1.5 is the release where Golem becomes agent-first across every surface, 1.6 is the release where the runtime layer asserts structural guarantees that change what kinds of agents you can safely build — and what kinds of multi-tenant systems you can build.

**Trust no text.** What an agent can do is determined by which system calls it uses — the actual "type signature" of the component — not by metadata it self-asserts or conventions written into its code. The runtime trusts the type signature.

**Possession-based authority.** Permissions become first-class resource handles, minted by the runtime. Agents cannot fabricate them. Derivation only narrows. The runtime is the trust root.

**Replay-deterministic authorization.** Authorization decisions, tool calls, and middleware behavior reproduce exactly under replay — the same oplog mechanism that recovers a crashed agent reproduces, byte-for-byte, the authorization decision that allowed it to act. This includes across crashes, redeploys, and human-in-the-loop pauses.

**Agent Tools.** A typed, CLI-shaped tool model with parallel runtime-enforced middleware — bypass-proof by construction — secrets as first-class capability-gated resources, and bidirectional MCP integration.

**Agent Permissions.** A first-class authority model based on the commitments above, with hierarchical owner paths, derivation-as-attenuation, and replay-deterministic authorization.

**TypeScript SDK redesign.** A fluent-builder def/impl split that unifies agent and tool authoring, eliminates the bundling problem, and gains type-inferred state and compile-time HTTP route validation. Migration is mechanical; the WIT contract and manifest format are unchanged.

Alongside these, 1.6 brings durable streams and async/concurrent streams for token, audio, and eventually video streaming; upgrading scalability bottlenecks where relevant; performance work tuned for the cost profile of agentic workloads; and per-agent SQLite/Turso for trivially-deployed local agent state.

## Launch event & hackathon

If Golem 1.5 sounds interesting, I invite you to join us for the launch event on **Thursday, May 14th at 12:00 EDT** (16:00 UTC / 17:00 BST / 18:00 CEST). We'll stream across LinkedIn, X, and YouTube.

Alongside the launch, we're kicking off **The Golem Forge Hackathon** with a $2,500 cash prize for the most extraordinary agent built on Golem 1.5. Theme, rules, and judging criteria will be announced live during the event.
