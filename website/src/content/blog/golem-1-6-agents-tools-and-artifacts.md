---
title: "Golem 1.6: Agents, Tools, and Artifacts"
date: "2026-10-09"
author: "John A. De Goes"
tags: ["Announcements", "Product Updates"]
slug: "golem-1-6-agents-tools-and-artifacts"
draft: false
---

On Monday, October 19th, we will release Golem 1.6, alongside a live launch event from 13:00 to 15:00 EDT.

In June, I argued in [The Rise of the Agent Runtime](/blog/the-rise-of-the-agent-runtime) that the dominant use of AI in 2026 is a coding agent — even for people who never see a line of code. An agent asked for a customer summary writes a small program, installs the packages it needs, runs it, inspects the output, and tries again. That makes a coding agent the most demanding agent there is. It needs a shell, a filesystem, tools, streaming output, long-lived state, sub-agents, and limits it cannot argue its way around — all at once.

Golem is an open-source, WebAssembly-based durable agent runtime. Agents survive crashes, redeploys, and time, and every effect they have on the world is journaled in an operation log, the _oplog_.

In May 2025, we [refocused Golem on agentic applications](/blog/golem-prepares-for-major-refocus-on-agentic-applications). Golem 1.3 and 1.4 made agents code-first. Golem 1.5, the agent runtime, made Golem agent-first across every surface. Internally, we set one bar for 1.6: it must be possible to build a complete coding agent entirely in Golem. Golem 1.6 clears that bar, and in doing so it completes the pivot.

With Golem 1.6, you deploy **agents, their tools, and their artifacts** to a single runtime. By _artifacts_ we mean the interactive applications, pages, and files that agents ship to the people they serve: HTML, CSS, and JavaScript front-ends, static assets, and live output, served by Golem and backed by the agent itself. All three are durable, governed by authority the runtime mints, and scaled by the same machinery. The harness — the loop that drives the model — chooses what an agent does; with 1.6, what an agent _can_ do is decided entirely by Golem.

The changes fall into five areas, and the rest of this post explains them in more detail:

- **Agents.** One authoring model across six SDKs — now including Go — streaming methods, Durable Streams, read-only methods, reflection, local retries, and faster state recovery.
- **Tools.** A typed tool model, host-enforced middleware, a standard toolkit, tool publishing, bidirectional MCP, and `golem ssh`.
- **Artifacts.** HTTP routers, static and live file serving, and front-end authentication.
- **Authority.** Trust no text, permission cards, opaque secrets, and replay-deterministic authorization.
- **Durability and scale.** A redesigned durability core on WASI P3, and a reworked cluster layer.

## Agents

### One model, six SDKs

The TypeScript SDK has been redesigned around a definition-and-implementation split. An agent is declared with `defineAgent`, using schemas from any Standard Schema library (Zod, Valibot, ArkType, or Effect Schema), and implemented separately. State types are inferred from `init`, and HTTP routes declared on methods are checked at compile time. Decorators, `BaseAgent`, and the type-generation packages are gone, and so is the bundling problem they caused.

```typescript
export const CounterAgent = defineAgent({
  name: "CounterAgent",
  id: { name: z.string() },
  methods: {
    increment: method({ input: {}, returns: z.number() }),
  },
});

export const CounterAgentImpl = CounterAgent.implement({
  init: () => ({ count: 0 }),
  methods: {
    increment() {
      this.count += 1;
      return this.count;
    },
  },
});
```

Golem now ships six SDKs across five languages. New in 1.6, the Go SDK 0.1.0 brings Golem to Go: agents defined with generics, definitions and implementations in separate packages so agents can call each other with typed clients, and the same tools, streams, durability, and HTTP routers as every other SDK. The Effect SDK, built on Effect 4.0, is developed and released as part of Golem, with native Effect streams, tools, reflection, and HTTP routers. Alongside the Go SDK 0.1.0, the TypeScript SDK 2.0, and the Effect SDK 2.0, the Rust SDK 3.0, Scala SDK 2.0, and MoonBit SDK 0.6 share one agent model, one schema system (now with first-class UUIDs), and one component format that exports agents, tools, and middleware together. Typed client generation covers every SDK, both for applications calling Golem from outside and for agents calling other agents and tools. The TypeScript REPL works against agents written in any of them.

### Streaming methods

Agent methods can take and return native streams of values or bytes, at any depth, so binary payloads such as audio stream the same way as tokens. Invocations run as resumable sessions over a public WebSocket protocol, so a client that disconnects mid-response can reconnect and continue, and a crash on the server does not lose the stream. The CLI gained matching session flags on `golem agent invoke`. Streaming methods work in every SDK and in generated bridges.

### Durable Streams

A streaming method mounted on an HTTP route is now exposed as a resumable stream URL, with catch-up reads, long-polling, server-sent events, idempotent producers, expiry, and forks. Golem implements the open [Durable Streams](https://github.com/durable-streams/durable-streams) protocol on these routes, and the unmodified reference client can read, write, and subscribe to them. This is what token streaming to a browser should look like: a tab that reloads picks up exactly where it left off. Agents can also read from and append to external Durable Streams servers, with credentials passed as opaque handles.

### Read-only methods

Methods can now be declared read-only, with a cache policy: no caching, cache until the next write, or a time-to-live. The runtime enforces the declaration. A write, an outgoing HTTP request, or an RPC call from a read-only method traps before anything persists. Results are cached in the executor and at the HTTP edge with strong ETags, so a repeated read can be answered with `304 Not Modified` without waking the agent or waiting behind its other work.

### Reflection

Agents can discover agent types and tools at runtime and invoke them through discovered or fully dynamic clients, alongside the generated typed clients. This is the foundation for orchestration patterns in which a parent agent decides, at runtime, which sub-agents and tools a task requires.

### Local retries

In 1.5, SDKs could scope a runtime retry policy to a block of code. Now every SDK can also execute the same policies locally, retrying your own code and typed failures directly. Use them when a retry loop belongs to your logic rather than to the runtime.

### State

Long-lived agents now update and recover without replaying their setup. Snapshot restoration has been redesigned so agents restore without re-running initialization, automatic updates can use snapshots to move agents to a new component revision, and reverts work across successful snapshot-based updates. Custom multipart snapshots let an agent save structured state alongside named binary parts, and Golem can now snapshot an agent's filesystem together with its state.

TypeScript agents already had embedded SQLite. In 1.6, Effect and Rust agents gain documented support for it too, along with an experimental in-memory Turso integration for Rust. Scala and MoonBit agents are not covered yet.

## Tools

Tools are an agent's hands. They are now deployables in their own right, and the host decides how they are called.

### The tool model

A tool is defined once, in any SDK, with typed, CLI-shaped metadata: commands, positional arguments, options, constraints, streams, results, and error cases. Models have seen this shape in thousands of examples in their training data, which makes tools easy for them to use correctly. Every tool invocation runs in a fresh, isolated instance owned by the calling agent — sharing the agent's filesystem only when a binding allows it — with durable standard input, output, and error. Typed tool clients are generated for every SDK.

### Middleware

Tool middleware is a transformation from tool to tool, enforced by the host on every tool call. There is no code path around it, whoever — or whatever — wrote the agent's code. Middleware runs in a defined order: environment-wide middleware first, then per-tool middleware, then the tool itself. Each middleware installation carries its own secret scopes, so an approval gate, a redaction filter, a rate limiter, or a path policy sees what it needs and nothing more. Middleware behavior is recorded in the oplog and reproduces under replay.

### The standard toolkit

Golem 1.6 ships a set of built-in tools: `bash`, `read-file`, `write-file`, `edit-file`, `ls`, `grep`, `git`, `node`, `npm`, `npx`, `tsc`, and `web-fetch`, plus a `path-policy` middleware for the filesystem tools. The `bash` tool runs a bash-compatible shell in-process, inside the sandbox, and the JavaScript tools run Node-compatible code, npm, and the TypeScript compiler without leaving Golem. The `git` tool works on local repositories today; clone, fetch, and push come next. Built-in tools are pinned to exact versions and configured per agent in the manifest:

```yaml
tools:
  edit-file:
    release: { account: builtin-tool-owner@golem.cloud, name: edit-file, version: 0.4.1 }
  npm:
    release: { account: builtin-tool-owner@golem.cloud, name: npm, version: 10.9.9+golem.2 }
  middleware:
    path-policy:
      release: { account: builtin-tool-owner@golem.cloud, name: path-policy, version: 0.1.1 }

agents:
  ReportAgent:
    tools:
      edit-file: { filesystemAccess: allowed }
      npm: { filesystemAccess: allowed }
```

### Tool publishing

Agents stay custom to each deployment, but the tools they use increasingly trade across organizational boundaries. You can now publish a tool as an immutable release and grant it to other environments and other accounts, which consume it by exact version. Tools can also be invoked directly, on an existing agent or on a fresh ephemeral one, through the REST API or `golem tool invoke`.

### MCP, both directions

Any agent could already be exported as an MCP server. Tools can now be exported too. And MCP can be imported: upstream MCP servers are declared in the manifest, their tools are projected into Golem's tool registry, and calls go through a durable bridge, with OAuth consent handled by the CLI and typed clients generated for agent code.

### `golem ssh`

For any agent with the `bash` tool, `golem ssh` opens an interactive prompt inside that agent's sandbox, with completion, per-agent history, and — when the `git` tool is bound — the current branch in the prompt:

```bash
golem ssh 'ReportAgent("q3")'
```

Each command is one call of the agent's `bash` tool, the call `golem tool invoke` makes, so bindings, middleware, and permission cards (covered below) apply to it as to any other.

## Artifacts

Agents produce things people use: front-ends, reports, live output. Artifacts in Golem don't sit next to agents. They are served by the runtime that runs them, from the same deployment, under the same authority, with the same audit trail.

### HTTP routers

An HTTP router is a new kind of agent that owns a mount point. It can provide a streaming handler for any Fetch-compatible framework, an OpenAPI document published alongside the domain's generated API description, or both. Routers are available in every SDK, including an integration with Effect's `HttpRouter`:

```typescript
export const EchoRouter = defineHttpRouter("EchoRouter")
  .mount("/echo")
  .implement((request) => new Response(request.body));
```

### Static and live files

A router can serve immutable static files that are versioned with the deployment and read directly from storage, without waking an agent. Any durable agent can also publish files from its own filesystem through GET and HEAD mounts: a report it just wrote, a build it just produced, a workspace it is editing. Both kinds of file mapping support custom response headers. Deployments can claim a subdomain, locally and in Golem Cloud, so an agent's front-end has a real URL from the first `golem deploy`, and application versions can be resolved from git.

### Front-end authentication

Single-page applications can now authenticate users with the OAuth2 authorization-code flow with PKCE, using short-lived bearer tokens issued by Golem. Per-mount authentication and CORS, introduced in 1.5, apply as before. From there, the front-end calls agents through code-first routes and receives streamed output through Durable Streams.

## Authority

In the [1.5 announcement](/blog/golem-1-5-the-agent-runtime), we committed to three structural guarantees: trust no text, possession-based authority, and replay-deterministic authorization. All three ship in 1.6.

### Trust no text

A component's WebAssembly imports are its capability declaration. A tool without filesystem imports cannot touch a filesystem, and a tool without the secret-reveal import cannot see plaintext, whatever its metadata claims. Bindings can only narrow that surface, never widen it: the effective capability is always an intersection.

### Permission cards

Authority in Golem is now carried by permission cards: unforgeable handles minted by the runtime, not assembled by the agent. Each card has two bounds: what its holder may do now, and an upper bound on what its holder may ever do, no matter what other cards it acquires. Deriving a card can only narrow it, and revoking a card revokes everything derived from it. Grants name the owner of a resource and the recipient of the authority, scoped anywhere from a whole account down to a single agent type. Cards pass between agents as typed values, scope individual RPC calls, and are installed into new agents from the manifest. The `golem card` command manages them.

A card with no network egress removes the exfiltration leg of the [lethal trifecta](https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/). To be precise about what this buys: a card bounds the blast radius, not the behavior. An agent can still misuse authority it legitimately holds, which is why irreversible effects deserve approval, and why approval gates belong in middleware.

### Opaque secrets

Secrets are now opaque handles. An agent can pass a handle to a tool, or to an external service integration, without ever seeing the plaintext. Plaintext reaches only code that holds explicit reveal authority, and every reveal is pinned to a secret revision, so retries and replays stay deterministic.

### Replay-deterministic authorization

Card events, tool calls, and middleware outcomes are recorded in the oplog. When an agent recovers — after a crash, a redeploy, or a human-in-the-loop pause — the mechanism that restores its state also reproduces the authorization decisions that let it act. The audit trail is not a separate system you configure. The runtime produces it as a side effect of running the agent.

## Durability and scale

Much of the work in 1.6 sits below the features above.

### Durability core

Every tool call executes exactly once, and a stale executor cannot write to an agent it no longer owns. Behind those two guarantees:

- **One durability level.** Durable agents no longer offer persistence levels; a durable agent is durable all the way through. The custom durability API has been redesigned around a single entry point.
- **Concurrent durability.** Host calls can now overlap durably. Agent-to-agent calls and tool calls execute exactly once.
- **WASI P3.** Golem now runs on WASI P3 and Wasmtime 46, with an asynchronous component model and streaming HTTP.
- **Fenced oplog writes.** Oplog writes are fenced by shard epoch, so an executor that has lost ownership of an agent cannot write to it.
- **Tracing.** Span lifecycles are embedded in durable operations, and the bundled OpenTelemetry exporter has been updated to match.

### Cluster layer

The shard manager has been reworked around executor-held shard leases, compare-and-swap persistence, leader election, and an etcd backend for distributed deployments. The invocation hot path does less copying and less allocation, and idle agents continue to suspend to zero with their full state intact. Storage and memory are now metered, with per-agent limits and `golem account usage` to see where they go, complementing the user-defined quotas introduced in 1.5.

You will never see a shard lease in a demo. You would notice the day it was missing.

## Business coding agents, entirely in Golem

The coding agents that matter most are the ones business users never know they are using: a support bot that writes a script to reconcile an account, an analyst's assistant that assembles a report from three systems, an operations agent that transforms a spreadsheet the same way every week.

The common pattern in the industry is to run the agent's harness in one place and its code in a separate Linux VM. The reasons are sound for conventional infrastructure: credentials should not share a space with model-written code, a crashed container should not take the session with it, idle VMs cost money, and guardrails and audit logs should be out of the agent's reach.

On Golem, none of these reasons apply. Model-written code runs in isolated tool instances, secrets are opaque handles, and authority comes from cards the runtime mints. Agents recover from the oplog, idle agents use no memory, middleware cannot be tampered with, and the audit trail is the oplog itself. There is no VM to boot.

Keeping a VM anyway is not free. A Linux VM is not durable: even with a durable harness, the VM can lose state and drift out of sync with the agent that drives it. And every heavyweight VM adds cost and latency to work that does not need it.

In March, we [built a coding agent on Golem](/blog/golem-as-a-coding-agent-engine) that had to emulate a shell, hand-write its file tools, shell out to the host for builds, and tail logs to show progress. Since then, we built a TypeScript coding agent internally that runs 100% inside Golem, including `npm`, `npx`, and `tsc`, and we will open source it. It streams its work, uses the standard toolkit under `path-policy` middleware and a card whose only egress is the model provider, and ships a web front-end as its artifact.

Some projects need native toolchains, such as compilers, containers, or full browsers, that cannot run in a WebAssembly sandbox. For those, Golem can host the durable harness and drive a separate VM as a tool. That pattern is supported. But for the business problems most coding agents will actually solve, an agent that runs entirely in Golem is simpler and cheaper, and it is more reliable.

## Upgrading to 1.6

Golem 1.6 is a major step, and some of it is breaking:

- Components must be rebuilt with the 1.6 SDKs.
- Manifests upgrade automatically from 1.5.0, except bridge configuration, which moves under `external` by hand.
- TypeScript agents move from decorators to `defineAgent`.
- Scala 2.13 support is dropped; the Scala SDK targets Scala 3.
- Rust value derives now come from the new `golem-schema` crate.
- REST clients and oplog consumers will see new payload shapes, and operators will find renamed configuration sections, such as `[active_workers]` becoming `[active_agents]` (see the migration guide).
- Persistence levels and environment shares have been removed, replaced by full durability for durable agents and by permission shares.
- The debugging service has been removed. The oplog remains fully inspectable through the CLI and REST API, and we intend to build visual debugging on top of it.

In the 1.5 announcement, we said the TypeScript redesign would leave the WIT contract and manifest format unchanged. We were wrong: the new schema model, together with tool and middleware exports, required changing both. A migration guide will accompany the release on October 19th.

## Looking ahead

Our 1.6 release completes the pivot. What comes next deepens it.

- **Per-agent graph memory.** A private graph and transactional store for every agent, suspended and resumed with the agent.
- **Beyond the sandbox, still under capability.** First-class support for native toolchains in VMs, driven as tools and governed by the same cards and middleware.
- **Remote git tool.** Clone, fetch, and push for the built-in `git` tool, over durable HTTP.
- **Better debugging and analysis.** Agent-native tooling to inspect, understand, and revert what agents did, built on the oplog.
- **Higher-level SDKs.** Provider-swappable model inference with usage tracking and quota enforcement powered by Golem.
- **More tools & agents.** Off-the-shelf, open source tools and agents you can use and customize for every scenario.
- **Higher performance.** Lower latency, increased throughput, and scaling to larger cluster sizes.

## Launch event

If Golem 1.6 sounds interesting, I invite you to join us live on **Monday, October 19th, 13:00–15:00 EDT** (17:00–19:00 UTC / 18:00–20:00 BST / 19:00–21:00 CEST), streamed on LinkedIn, X, and YouTube. We will kill a server mid-task and watch a coding agent carry on.

In the meantime, the release candidate is available on [GitHub](https://github.com/golemcloud/golem/releases), the [documentation](https://learn.golem.cloud) covers the new features, and our [Discord](https://discord.gg/UjXeH8uG4x) is the best place to ask questions.
