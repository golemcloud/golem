---
title: "What's This Golem Cloud Thing About?"
date: "2026-10-11"
author: "John A. De Goes"
tags: ["AI Agent", "Durable Computing", "Engineering Articles"]
slug: "whats-this-golem-cloud-thing-about"
description: "A guided tour of Golem 1.6: durable, isolated agents, their tools, and their artifacts on one runtime, and how it compares to the rest of the agent stack."
image: "/blog-images/whats-this-golem-cloud-thing-about-social.png"
draft: false
---

Building an agent that works in a demo is easy. Keeping it working is the hard part. An agent dies at step 7 of 12, starts over from step 1, and pays for every token again. A manager approves a refund, but the pod holding the waiting agent was redeployed overnight, and the approval goes nowhere. An agent deletes a production database during a code freeze, because the freeze existed only as a sentence in its prompt.

The industry now describes an agent as three parts: a model, a **harness** (the loop that drives the model and carries out its tool calls), and the infrastructure underneath. That infrastructure is where these failures live, and today it is assembled from queues, state stores, retry wrappers, idempotency tables, a sandbox VM, a permission layer, and a lot of glue.

Golem is that infrastructure, as one open-source runtime, and your harness runs inside it. You write an agent as ordinary code, in TypeScript (plain or with Effect), Rust, Scala, MoonBit, or Go, and deploy it to Golem. From then on, every agent you address by name is a small, isolated, durable computer, with its own memory, its own filesystem, and its own permissions. It survives crashes, redeploys, and months of waiting, and nothing it has already done happens twice.

Around those agents, Golem runs the **tools** they use (a shell, files, git, Node, npm, TypeScript, the web) and the **artifacts** they ship to people (web front-ends, APIs, files, live streams). All three run on one runtime, under one permission model, with one audit trail.

Golem is the runtime, and it runs on your laptop or your own servers. **Golem Cloud** is the hosted service that runs it for you. Everything in this post works the same on all of them. By the end, you'll see a coding agent that runs `npm install` and `tsc` inside Golem, with no VM, and carries on when the node under it dies halfway through.

![The Golem runtime: artifacts (HTTP routes, web pages, live files, MCP server, streams) served by durable agents, each with its own memory, files, cards, and oplog, which use tools such as bash, git, node, npm, tsc, web-fetch, and MCP behind host-enforced middleware; every effect is journaled, checked against cards, and spread across shards](/blog-images/whats-this-golem-cloud-thing-about-01-runtime-overview.webp)

The *oplog* is each agent's journal of everything it did, *cards* are its permissions, *middleware* is policy the host runs around every tool call, and *shards* are how agents spread across machines. The rest of this post takes that picture apart, one box at a time: what each piece is, how it works, and why you should care.

## If you already know...

If you've built with other tools, here is your starting point:

| Coming from | Familiar | What changes |
|---|---|---|
| **Durable execution** (Temporal, Restate, Inngest, DBOS) | A journal of steps, replayed after a crash | Every host call is a step, with no annotations or determinism rules in your code |
| **Actors** (Orleans, Akka, Rivet, Cloudflare Durable Objects) | Addressed by key, one message at a time, asleep when idle | Nothing to save: effects are journaled, memory is rebuilt by replay, and recovery resumes mid-method |
| **Sandboxes** (E2B, Modal, Daytona) | An isolated workspace for model-written code | Each component tool call is a fresh WebAssembly instance that can work on the agent's durable disk |
| **Agent frameworks** (LangGraph, OpenAI Agents SDK, Vercel AI SDK) | Your harness: the loop, prompts, and tool calling | Golem is the runtime underneath: exactly-once tool calls, zero-cost waits, and host-enforced policy. Harnesses in any SDK language run inside; Python harnesses call in over MCP, REST, or HTTP endpoints |

If your stack speaks of sessions, checkpoints, guardrails, and handoffs: a session is an agent, a checkpoint is the oplog, a guardrail is host-enforced middleware, and a handoff is a typed agent-to-agent call, executed exactly once.

## The agent: a tiny, isolated, durable computer

Start with the middle box of the picture: the agent.

A note on the word *agent*. In Golem, an agent is any named, stateful object: a user, an order, a chat, a coding session. LLM-driven agents are one important kind, and the runtime treats them like every other, so the counter below and the coding agent at the end of this post are the same kind of thing.

### Agent types and agents

You define an **agent type** once, and Golem runs any number of **agents** of that type. Think of an executable and its processes. Here is a complete agent type in TypeScript:

```typescript
import { z } from "zod";
import { defineAgent, method } from "@golemcloud/golem-ts-sdk";

export const CounterAgent = defineAgent({
  name: "CounterAgent",
  id: { name: z.string() },              // identity = constructor parameters
  methods: {
    increment: method({ input: {}, returns: z.number() }),
    add: method({ input: { by: z.number() }, returns: z.number() }),
  },
});

export const CounterAgentImpl = CounterAgent.implement({
  init: () => ({ count: 0 }),             // initial state, bound to `this`
  methods: {
    increment() { this.count += 1; return this.count; },
    add({ by }) { this.count += by; return this.count; },
  },
});
```

There is no database here, no save call, and no session store. `this.count` is the state, and Golem keeps it.

Agent types are compiled into a **component**, a WebAssembly component that is the unit you build and deploy, a bit like one container image holding several services. Everything that isn't code, such as files, tool bindings, permissions, HTTP and MCP deployments, and policies, lives in the app's manifest, `golem.yaml`. The manifest also defines the **environments** you deploy into, such as `local` and `prod`.

### The name is the address

An agent's identity is its type plus its constructor parameters. `CounterAgent("alice")` and `CounterAgent("bob")` are two different agents, and every call to `CounterAgent("alice")`, from anywhere, reaches the same one. An agent is **created on first use**. You don't register it, provision it, or route to it. You call it. Then, if you like, you crash it:

```shell
golem agent invoke 'CounterAgent("alice")' increment   # creates alice, returns 1
golem agent invoke 'CounterAgent("alice")' increment   # same agent, returns 2
golem agent simulate-crash 'CounterAgent("alice")'     # kill it
golem agent invoke 'CounterAgent("alice")' increment   # 3: nothing lost, no database
```

`simulate-crash` takes the same recovery path as a real crash, which we'll walk through shortly, so you are testing the real thing.

Each agent processes one invocation at a time, so agent code needs no locks. Further calls queue in order; a cached read-only method answers without waiting, and a running invocation can be interrupted, both of which come up later. When you want parallelism, you use more agents.

If you know actors, the difference is that nothing is written back and there is no checkpoint or recovery hook to code: Golem rebuilds memory by replay, with locals and call stack intact. Each agent is also a sandboxed computer with its own cards.

Who an agent *acts as*, identity in the security sense, is a separate question, answered by its cards and the caller's principal; we'll get to both.

### Durable, ephemeral, and phantom

| Kind | What you get | Use it for |
|---|---|---|
| **Durable** (default) | State persists across invocations, crashes, and restarts. Every effect is journaled. | Almost everything: users, sessions, orders, conversations |
| **Ephemeral** (`mode: "ephemeral"`) | A fresh agent per invocation; state is discarded afterward | Stateless request handlers, serverless-style functions |
| **Phantom** | Many independent durable agents that share the same constructor values, told apart by a UUID | Parallel runs of the same agent, such as ten `ResearchAgent("pricing")` jobs at once, without inventing an ID for each |

Durable and ephemeral are modes you set on the type. Phantom is a way of addressing agents, chosen by the caller: `client.newPhantom(...)` creates one and returns a client for it together with its UUID. A durable counter incremented three times returns 1, 2, 3. The same code marked ephemeral returns 1, 1, 1.

### What is yours and what is shared

Each agent gets its own:

- **WebAssembly instance and memory.** No other agent can read or write it.
- **Filesystem.** An agent cannot reach any file outside its own filesystem.
- **Environment variables and configuration.**
- **Oplog**, its durable history.
- **Wallet of permission cards**, the only source of its authority. A card is a set of grants minted by the runtime, such as "may connect to `api.partner.com:443`" or "may delete under `/workspace`". Without a card that covers it, an effect is denied. Every agent starts with a card, broad by default, as we'll see when we get to cards.

Agents talk to each other only through their typed public methods. One agent's failure does not affect another's health.

What *is* shared is precise. Agents of the same component share the compiled code, read-only, so it is loaded once. Beyond that, agents share only what you deliberately choose to share: an external PostgreSQL or MySQL database, or a key-value or blob bucket opened by name in the same environment. Those, too, are checked against each agent's cards.

### WebAssembly, not VMs

Each agent is a WebAssembly instance, not a container or a virtual machine. That one decision explains why Golem can afford to give every user, every conversation, and every task its own computer:

![A container or VM per agent stacks your code on a language runtime, libraries, a guest kernel, and an image with boot and idle costs; Golem agents are just your code and linear memory, reaching a shared executor, which loads compiled code once, through host imports only](/blog-images/whats-this-golem-cloud-thing-about-02-wasm-vs-vm.webp)

A WebAssembly instance has no kernel to boot and no image to pull. Its memory is a bounds-checked region, and it can touch the outside world only through imports the host provides. An instance with no filesystem import cannot reach a filesystem, whatever its code tries. The cost of an agent is essentially its memory, and an agent that is waiting has none (more on that below). A warm call to an agent takes a few milliseconds end to end, and even the first call to a component the node has never seen completes in well under a second.

Agents are Wasmtime instances in a shared executor process, so the boundary between them is WebAssembly's memory safety and that import surface, not a separate kernel. There is no ambient authority: every gated import is checked against the agent's cards. It is a different wall from a microVM's, and the two compose: self-hosters can run executors inside VMs, which shield the host if WebAssembly is ever escaped. Like a V8 isolate, an instance starts almost instantly, but every host call is journaled, and every gated one is checked against cards.

So what runs inside? TypeScript and Effect agents, and Scala agents through Scala.js, run on a JavaScript engine built on QuickJS and compiled to WebAssembly, not on Node.js. It provides `fetch`, web streams, and Node-compatible core modules, including `http`, `net`, `crypto`, and `zlib`, so most npm packages written in plain JavaScript work as they are; packages that need native C++ addons don't. Rust, Go, and MoonBit agents compile straight to WebAssembly and reach files, clocks, and the network through WASI. No agent, in any language, can spawn host processes or load native libraries; the shell, Node, npm, and `tsc` an agent needs come as WebAssembly tools instead, as we'll see when we get to tools.

### A filesystem and a database of its own

Every agent has a real filesystem. In TypeScript you use `node:fs`; in Rust, `std::fs`. You can seed every new agent with files from the manifest, per component or per agent type:

```yaml
components:
  acme:assistant:
    files:
      - sourcePath: ./prompts/system.md
        targetPath: /prompts/system.md
        permissions: read-only
```

Files are durable from the agent's point of view: after a crash or a move to another node, the agent's files are there.

Agents written in **TypeScript, Effect, and Rust** also get **embedded SQLite**, one database per agent, in memory or in a file on the agent's own disk:

```typescript
import { DatabaseSync } from "node:sqlite";

export const NotesAgentImpl = NotesAgent.implement({
  init: () => {
    const db = new DatabaseSync(":memory:");
    db.exec("CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT NOT NULL)");
    return { db };
  },
  methods: {
    add({ text }) {
      this.db.prepare("INSERT INTO notes (text) VALUES (?)").run(text);
      return Number((this.db.prepare("SELECT count(*) AS n FROM notes").get() as { n: number }).n);
    },
  },
});
```

For data that many agents share, every SDK can use external PostgreSQL and MySQL, and most can use Apache Ignite.

### Limits and suspension

Each agent runs inside per-agent memory and disk limits, and its CPU use is metered, so one runaway agent cannot starve its neighbors.

When an agent is waiting, for a timer, a promise, or a slow call to another agent, Golem **suspends it to zero**. After a short threshold (about ten seconds for sleeps and promises), the agent is unloaded from memory entirely. Only its durable state remains: its oplog, its files, and any snapshots. When the timer fires or the answer arrives, Golem rebuilds the agent exactly where it was and carries on. Idle agents that are merely resident are evicted under memory pressure the same way.

That is what turns one agent per user, per chat, or per order into the normal design rather than an extravagance. The name is the address, state is just variables and files, and isolation is the runtime's job, not yours.

## Durable by default

So far, we've claimed that an agent survives crashes and never does anything twice. Here is how, because almost everything else in Golem is built on it.

### The oplog

Every durable agent has an **oplog**, an append-only journal of everything that happens to it. When your code touches the outside world (reads the clock, generates a random number, calls an HTTP API, calls another agent, calls a tool), Golem writes down what was asked and what came back. Invocations, updates, snapshots, and permission events go in the same journal.

Think of it as a flight recorder that also flies the plane. If the machine dies, Golem doesn't try to rescue the dead process. It starts a fresh instance of your code and lets it run from the top. Every time the code reaches a step that is already in the oplog, Golem hands back the recorded answer instantly instead of doing it again. Within moments, the program is back at the exact line where it stopped, with the same variables, the same call stack, and the same files, and it simply continues.

Long-lived agents also take **snapshots** of their state, on a policy you choose, so recovery starts from the latest snapshot and replays only what came after, even for an agent alive for months. Replay is not a per-call cost: it runs only when an agent is rebuilt or updated, and it is fast, because every effect is answered from the oplog and only your computation re-runs.

### A crash, step by step

An `OrderAgent` takes a payment over HTTP, reserves stock in an `InventoryAgent`, and returns.

![Crash and recovery of OrderAgent("order-42"): oplog entries #20 to #24 are recorded, the node crashes, and recovery re-runs the code from #21, answers the HTTP call at #22 from the oplog without sending it, and re-sends the call at #24 with the same key so InventoryAgent returns its recorded result; the caller gets one answer](/blog-images/whats-this-golem-cloud-thing-about-03-crash-recovery.webp)

Nothing that already happened happens again, and nothing in flight is lost. Had the crash come between #22 and #23, after the charge was sent but before its answer was recorded, Golem would resend the request with the same idempotency key, so a payment provider that honors the key charges once and returns the original result. A node that has lost ownership of an agent cannot write to its oplog, so there is never a second copy racing the first. Suspension, eviction, rebalancing, restart, and crash all use this same path to rebuild an agent, which is why recovery is ordinary rather than exceptional.

### The same, for a model call

This LLM agent keeps its conversation in `this.messages` and calls a model provider with plain `fetch`:

```typescript
export const ChatAgentImpl = ChatAgent.implement({
  init: () => ({ messages: [] as { role: string; content: string }[] }),
  methods: {
    async ask({ question }) {
      this.messages.push({ role: "user", content: question });
      const response = await fetch("https://api.model-provider.example/v1/chat", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ messages: this.messages }),
      });
      const reply = (await response.json()).message;   // journaled in the oplog
      this.messages.push(reply);
      return reply.content;
    },
  },
});
```

If the node dies after the model has answered, recovery reads that answer from the oplog instead of requesting it, and paying for it, again, and `this.messages` comes back exactly as it was. A twelve-step agent loop that dies at step 7 resumes at step 7, and steps 1 to 6 are not paid for again.

### What "durable" means, precisely

*Durable* usually means one of two things: durable state, as in actors, or durable steps, as in workflow engines, offered as two primitives with different rules. Golem has one primitive for both, the oplog. It is the journal you know from durable execution, moved to the WebAssembly host boundary, so every effect is a step automatically, with no step annotations and no activity wrappers. It gives you three things.

**State persists automatically.** Memory is not dumped to disk on every step. It is reconstructed by re-running your code against the recorded answers. There are no determinism rules because a WebAssembly instance reaches the outside world only through host calls, and Golem journals every one, including the clock, randomness, and all I/O. Each agent is single-threaded, and even the order in which concurrent calls complete is journaled, so execution order is fixed too. Replay is faithful by construction, not by discipline. Locals, closures, the heap, and the call stack all come back. A plain `this.count += 1` is durable. The one place replay constrains you is changing code under a live agent: an automatic update whose code diverges from the recorded history fails safely, and a manual update is the way out, as we'll see later.

**Accepted work runs to completion.** Once Golem accepts an invocation, it runs to completion, across any number of node failures, restarts, or updates. This is not automatic rollback: completed steps stay completed and execution resumes forward. When you want all-or-nothing behavior, you ask for it with atomic regions or sagas (below).

**Everything inside Golem executes exactly once.** That holds because both ends are Golem: every retry and replay carries the original key or oplog position, and the recorded result comes back instead of a second execution. Where a call leaves Golem, the outgoing HTTP row below applies.

| Interaction | Guarantee |
|---|---|
| Agent invocation | Executes exactly once per idempotency key; a repeat with the same key returns the recorded result |
| Agent-to-agent call | Executes exactly once: every retry and replay of the call carries the same key, and the target runs it once |
| Tool call | Executes exactly once: a completed call is never performed again, and an interrupted call is finished as that same call, never started over as a second one (the section on tools shows how) |
| Your own code | No effect your code performs is repeated; replay re-runs only the computation in between, using the recorded answers |
| Outgoing HTTP | Completed requests are never re-sent; Golem attaches an `idempotency-key` header to every outgoing request automatically, stable across retries and recovery, so services that honor it see each request once |

You don't generate idempotency keys or keep dedupe tables: Golem derives one from the agent's oplog position. If you need the key in a request body instead, `generateIdempotencyKey()` gives you a durable one.

Agent frameworks usually ask something different of you on resume:

| Usually, on resume | On Golem |
|---|---|
| Tools may rerun; make them idempotent | Every tool call executes exactly once |
| A resumed step restarts from the top | The agent resumes at the line where it stopped |
| Model calls may be re-sent and re-paid | Completed model calls are never re-sent |
| Subagent handoffs may run twice | Agent-to-agent calls execute exactly once |

### Retries are policies, not loops

Golem retries transient failures for you: network errors, 5xx responses if you opt in, and crashes in your code. Retry policies are named, prioritized, and composable, and you can declare them per environment in `golem.yaml`:

```yaml
retryPolicyDefaults:
  prod:
    http-5xx-retry:      # retry 500, 502, 503, or 504 up to 3 times, after 200ms, 400ms, 800ms
      priority: 20
      predicate:
        propIn: { property: "status-code", values: [500, 502, 503, 504] }
      policy:
        countBox:
          maxRetries: 3
          inner:
            exponential: { baseDelay: "200ms", factor: 2.0 }
```

You can also change them live with `golem retry-policy create|update|delete`, with no redeploy; running agents pick them up immediately. Code can scope a policy to a block with `withRetryPolicy`.

### Atomic regions and sagas

By default, effects are journaled and retried one by one. When several effects belong together, wrap them:

```typescript
import { atomically } from "@golemcloud/golem-ts-sdk";

const payment = await atomically(async () => {
  const response = await fetch("https://payments.example.com/charge", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ orderId, amount }),
  });
  if (!response.ok) throw new Error(`payment failed: ${response.status}`);
  return await response.json();
});
```

If the region is interrupted, by a crash or a retryable error, the whole region re-executes from its start, as you asked, re-sending even its completed requests with the same idempotency keys, so services that honor them still see each request once. Business transactions that need undo after a domain failure, such as a declined payment, use `compensable(execute, compensate)` and `fallibleSaga`: they run steps and, on that failure, run the compensations in reverse order.

Most "reliable" agent stacks are mostly plumbing: queues, outboxes, state stores, retry wrappers, idempotency tables, and workflow DSLs. On Golem, ordinary code is the durable workflow, and the plumbing is the runtime's problem.

## Agents working together

One durable agent is useful. Real systems are many of them, calling each other, waiting on the outside world, and streaming results, and all of it inherits the guarantees we just covered.

### Typed calls between agents

Defining an agent defines a typed client for it. Calling another agent looks like calling a method on a local object:

```typescript
const london = WeatherAgent.client.get({ location: "London" });

const report = await london.currentWeather();          // await the result
london.refresh.trigger();                              // fire and forget

const inOneHour = { seconds: BigInt(Math.floor(Date.now() / 1000) + 3600), nanoseconds: 0 };
const token = london.refresh.schedule(inOneHour);      // run later, durably
token.cancel();                                        // changed your mind
```

`get` creates the target if it doesn't exist. Every call executes exactly once, even if either side crashes halfway through. A scheduled call is itself durable, so a recurring task is just a method that schedules its own next run.

### Across components and languages

The agent you call can live in another component and be written in another language. Declare the dependency, and `golem build` generates a typed client package in your language, here `counter-agent-guest-client`, from the other agent's schema. Point a path alias at it in the consumer's `tsconfig.json`, and import it:

```yaml
components:
  acme:counter:
    templates: rust
  acme:consumer:
    templates: ts
    dependencies:
      agents:
        - acme:counter/CounterAgent
```

```typescript
import { CounterAgent } from "counter-agent-guest-client";

const counter = CounterAgent.get("my-counter");   // a Rust agent
const count = await counter.increment();          // called from TypeScript
```

Generated clients take constructor parameters positionally. This works because every SDK speaks one schema model, and the generator reads schemas, not source code. These **guest bridges**, generated client stubs much like gRPC stubs, exist for every SDK: TypeScript, Effect, Rust, Scala, MoonBit, and Go.

### Calling agents from ordinary programs

The same generator builds **external bridges**: typed client libraries for programs that don't run on Golem, such as a Node web server, a Rust service, or a Scala backend. Every SDK has them.

```yaml
bridge:
  ts:
    external:
      agents:
        - CounterAgent
```

```typescript
import { CounterAgent, configure } from "counter-agent-client/counter-agent-client.js";

configure({ server: { type: "local" }, application: "shop", environment: "local" });

const c1 = await CounterAgent.get("c1");
const value = await c1.increment();
```

Ordinary methods go over Golem's REST invocation API; streaming methods use a resumable WebSocket session (Node.js for the TypeScript bridges).

### Reflection

Sometimes a parent agent decides at runtime which sub-agents and tools a task needs. **Reflection** lets an agent list the agent types and tools visible to it, read their schemas (including JSON Schema you can hand to an LLM's tool-calling API), and call them through discovered or fully dynamic clients:

```typescript
import { getAllAgentTypes, getReflectedAgentType } from "@golemcloud/golem-ts-sdk";

const available = getAllAgentTypes();
const counterType = getReflectedAgentType("CounterAgent");
const schema = counterType!.method("add")!.input.toJsonSchema();
const result = await counterType!.client.get({ name: "main" }).method("add").invoke({ by: 5 });
```

The host stays in charge: reflection shows only what the caller may see, and every call is still authorized.

### Waiting is free: promises and webhooks

An agent that needs something from outside Golem, such as a human's approval or a payment provider's callback, creates a promise and awaits it. While it waits, it is suspended to zero, whether that is ten seconds or ten days, and a redeploy in between loses nothing: the wait lives in the oplog, not in a pod.

```typescript
import { createWebhook } from "@golemcloud/golem-ts-sdk";

async awaitPayment() {
  const webhook = createWebhook();
  await this.sendCheckoutLink(webhook.getUrl());  // hand the URL to Stripe or a person
  const payload = await webhook;                  // suspended here, at zero cost
  return payload.json();
}
```

A webhook is a promise with a one-time public URL. Plain promises (`createPromise`, `awaitPromise`, `completePromise`) can be completed by another agent or through the REST API. The `human-in-the-loop` template wires this up end to end.

This is what makes **background agents** and **ambient agents** practical: agents woken by a schedule, a webhook, or a promise, holding no process while they wait.

### Streaming

Methods can take and return native streams of values or bytes, at any depth, with back-pressure in both directions. Streams use the SDK's own schema markers, `s`, alongside your Zod schemas:

```typescript
// definition
methods: {
  doubled: method({ input: { values: s.stream(s.u32()) }, returns: s.stream(s.u32()) }),
},
```

```typescript
// implementation
doubled({ values }) {
  return AgentStream.from((async function* () {
    for await (const v of values) yield v * 2;
  })());
},
```

External clients consume streams through a resumable session: a client that disconnects mid-response reconnects and continues where it left off. If the server crashes, chunks already produced stay in the stream, and the recovered agent keeps appending. When we get to artifacts, you'll see the same streams served to browsers.

Distributed systems usually need queues, retries, a cron box, a callback router, and a client SDK per language, each of which can lose or duplicate messages. In Golem, these are method calls.

## Tools

Agents that call each other can coordinate. To do work in the world, they need tools. Tools are an agent's hands. An LLM agent that can only talk is a chatbot; give it a shell, files, git, Node, and the web, and it can do work. Golem 1.6 makes tools first-class deployables, with their own model, isolation, and policy.

### The CLI-shaped tool model

A Golem tool is described like a command-line program, with a type system: commands and subcommands, flags and options, positional arguments, stdin, stdout, and stderr, typed results, and declared errors with exit codes. Models have seen this shape in countless examples, which makes tools easy for them to use correctly. Golem turns one description into typed clients for every SDK, CLI help text, a REST endpoint, and an MCP tool.

```typescript
import { readFileSync } from "node:fs";
import { z } from "zod";
import { ok, toolDefinition } from "@golemcloud/golem-ts-sdk";

export const wc = toolDefinition("wc", { requiresFilesystem: true })
  .version("1.0.0")
  .body((body) => body
    .positional("path", z.string())
    .returns(z.number()));

wc.implement({
  wc: async ({ path }) => ok(readFileSync(path, "utf8").split(/\s+/).filter(Boolean).length),
});
```

An agent calls it through a typed client:

```typescript
import { toolClientDefinition } from "@golemcloud/golem-ts-sdk";

// client("wc") names the tool as this agent's binding calls it; .wc(...) runs its command
const words = await toolClientDefinition(wc).client("wc").wc({ path: "/workspace/draft.md" });
```

In an LLM loop, the harness, which in Golem is ordinary agent code, reads each bound tool's JSON Schema through reflection, hands it to the model as an ordinary tool definition, and runs whatever the model picks through the same typed client. Reach for a tool rather than an agent method when the operation is stateless and should run in a fresh sandbox, on the caller's disk, under the caller's authority.

### Isolated, but working on the agent's disk

Every call to a component tool (one compiled to WebAssembly, as opposed to an imported MCP tool) runs in a **fresh, isolated WebAssembly instance**, owned by the calling agent and destroyed when the call ends. No memory survives between calls, and no instance is shared between agents. While it runs, a tool's own effects, such as its HTTP requests, clock reads, and random numbers, are journaled in the calling agent's oplog just like the agent's own, so a call interrupted by a crash is replayed in a new instance through its recorded steps and then continues from where it stopped.

When the agent's **binding** for the tool, its entry under the agent type's `tools:` in the manifest, allows it with `filesystemAccess: allowed`, that instance works directly on the **calling agent's own filesystem**: not a copy, the same disk. Without that setting, a tool never touches the agent's disk, and `filesystemAccess: denied` keeps it that way even when a broader binding would allow it. The tool declares that it needs a filesystem (`requiresFilesystem`), and the binding decides whether it gets the agent's. This is what makes a coding agent possible: the agent writes a file, calls `tsc`, and reads the compiled output.

```typescript
// TscClient: a typed client for the built-in `tsc` tool
// root: the project directory; collect: reads a stream into a string
writeFileSync(`${root}/src/main.ts`, "export const main: number = 17;\n");

const run = TscClient.tsc({ cwd: root, args: ["--outDir", "out", "src/main.ts"] });
const [exitCode, stdout, stderr] = await Promise.all([
  run.result,
  collect(run.stdout!.getReader()),
  collect(run.stderr!.getReader()),
]);

const js = readFileSync(`${root}/out/main.js`, "utf8");   // the tool's output, on the agent's disk
```

Here is exactly what a tool call shares with its agent:

![What a tool call shares with CodingAgent("acme", "u-17"): never its memory; its filesystem only with filesystemAccess; its cards, narrowed; environment variables, filtered by cards; config and secrets, limited to listed keys; and every effect the tool performs is recorded in the agent's oplog](/blog-images/whats-this-golem-cloud-thing-about-05-tool-sharing.webp)

A tool acts with the calling agent's cards, narrowed by its binding. It can reach the network only if its component imports networking, and only where the agent's cards allow. A binding can narrow what a tool gets; it can never widen it. The effective capability is always an intersection, never a union.

Secrets are narrowed per binding: each lists which secrets the tool may read and which it may reveal in plaintext:

```yaml
agents:
  SupportAgent:
    tools:
      search:
        secretKeysReadable: "*"
        secretKeysRevealable:
          - credentials.search.token
```

### The built-in toolkit

Golem 1.6 ships a standard toolkit, every piece of it compiled to WebAssembly and running inside Golem, in the same sandbox as any other tool:

| Tool | What it is |
|---|---|
| `bash` | A bash-compatible shell (the Brush shell) with coreutils, `find`, `xargs`, `curl`, and `wget` built in; bound tools are callable as commands |
| `read-file`, `write-file`, `edit-file`, `ls`, `grep` | Read, write, edit, list, and search files |
| `node` | A Node-compatible JavaScript runtime |
| `npm`, `npx` | Upstream npm, installing packages over durable HTTP |
| `tsc` | The upstream TypeScript compiler |
| `git` | Git for local repositories: init, status, diff, log, branch, add, commit, checkout |
| `web-fetch` | A bounded, read-only HTTP GET |
| `path-policy` | Middleware (below): per-directory read, write, and delete rules for the filesystem tools |

So `npm install` and `tsc` run inside a WebAssembly sandbox, on the agent's durable disk, under the agent's permissions, with every call journaled. No container, no VM.

Nothing is bound automatically. You pin an exact release and bind it per agent:

```yaml
tools:
  bash:
    release: { account: builtin-tool-owner@golem.cloud, name: bash, version: "0.2.1" }
  tsc:
    release: { account: builtin-tool-owner@golem.cloud, name: tsc, version: "5.9.2+golem.4" }
agents:
  CodingAgent:
    tools:
      bash: { filesystemAccess: allowed, secretKeysRevealable: [] }
      tsc: { filesystemAccess: allowed, secretKeysRevealable: [] }
```

Each `bash` call starts a fresh shell. Only the working directory carries over, when the caller passes back the `cwd` the previous call returned; `golem ssh` does this for you.

`path-policy` governs the named filesystem tools; it is not a sandbox for `bash` or `node`. To bound those, use cards, which come next.

### Middleware the agent cannot touch

**Middleware** is a transformation from tool to tool, enforced by the host on every call. The operator configures it in the manifest; agent code cannot list it, reorder it, remove it, or call around it. Environment-wide middleware runs first, then per-tool middleware, then the tool. Each middleware installation lists its own readable and revealable secrets, just like a binding.

```typescript
import { ToolInvokeError } from "@golemcloud/golem-ts-sdk";

// `files` is a tool definition with a `read` command
export const safeFiles = files.middleware({
  name: "safe-files",
  implementation: {
    read: async ({ path, ...input }, { underlying }) => {   // underlying = the next middleware, or the tool
      if (path.startsWith("/etc/")) {
        throw new ToolInvokeError({ tag: "constraint-violation", val: "path denied" });
      }
      return underlying.read({ path, ...input });
    },
  },
});
```

Approval gates, redaction filters, audit hooks, and rate limiters are all middleware you can write. An approval gate runs inside the agent's durable execution, so it can wait hours for a human and survive restarts while it waits. Middleware outcomes are recorded in the oplog.

### Publishing, MCP, and `golem ssh`

**Publishing.** A tool can be published as an immutable release and granted to other environments and accounts, which consume it by exact version. Consumers see the definition, never the publisher's code, configuration, or files.

**MCP (Model Context Protocol), both directions.** A tool's commands can be exported as an MCP server from the manifest. Each MCP call runs as if made by a fresh, throwaway agent, with middleware applied. The other way, `mcp.imports` turns an upstream MCP server's tools into Golem tools with typed clients, with OAuth consent handled by the CLI; each call still executes on the upstream server.

```yaml
mcp:
  imports:
    local:
      - url: https://tools.example.com/mcp
        prefix: catalog
        include: ["search*"]
```

**Direct invocation.** `golem tool invoke --agent 'SupportAgent("acme")' search -- query "durable streams"` calls a tool on an existing agent, with all of that agent's bindings, middleware, and cards.

**`golem ssh`.** For any agent with `bash` bound, `golem ssh 'CodingAgent("acme", "u-17")'` opens an interactive prompt inside that agent's sandbox, with tab completion, and the git branch in the prompt when `git` is bound too. Each line you type is one `bash` tool call, so bindings, middleware, and cards apply exactly as they do for the agent. It looks like SSH, but there is no daemon and no terminal: each command's output arrives when it finishes.

### The execution ladder

An agent has three rungs to run code on. Climb only when you must:

![The execution ladder: first the agent's own code; then tools in WebAssembly such as bash, node, npm, tsc, git, and web-fetch, a fresh instance per call; and only when needed, a VM or remote sandbox driven as a tool for Python, apt, Docker, real browsers, GPUs, and native binaries](/blog-images/whats-this-golem-cloud-thing-about-06-execution-ladder.webp)

The result is that you can give an LLM real powers, including a shell, Node, npm, TypeScript, git, the web, and any MCP server, without giving it a VM. Every call is sandboxed, limited to the agent's own disk and authority, put through policy it cannot bypass, and journaled. Which raises the obvious question: who decides what that authority is?

## Authority: cards

LLM agents read text written by strangers, and any instruction hidden in a web page, an email, or a ticket can steer the model. Agents are also overeager (remember the code freeze that existed only in a prompt), they make honest mistakes, and people asked to approve every action soon approve nearly everything: approval fatigue. A rule in a prompt is not a security boundary. The sound advice is to enforce limits outside the agent, and Golem does: it moves the boundary into the runtime, where it bounds every agent's blast radius.

### The only source of permission

In Golem, authority is carried by **permission cards**. A card is an unforgeable handle minted by the runtime, never assembled by the agent, and it carries a list of **grants**; in security terms, it is a capability token. An agent's set of cards is its **wallet**, and every gated effect is checked against it: opening a file, connecting to a host, reading an environment variable or a secret, using a key-value store or a database, calling another agent, calling a tool.

![A permission check: agent code calls fetch on evil.example, the host checks the wallet for a network connect grant, and with no card covering evil.example:443 the request is denied and recorded in the oplog without ever leaving; when a card covers it, the request is sent and the response journaled](/blog-images/whats-this-golem-cloud-thing-about-07-card-check.webp)

An empty wallet means every gated effect is denied. The agent can still compute on its own memory and return results, but it cannot touch a file, a socket, a secret, or another agent.

Cards say who holds which authority. Middleware, which we met with tools, is the policy enforcement point for tool calls: it decides, call by call and argument by argument, what happens with that authority, such as holding any refund tool call over $500 for a human's approval. Capability-based designs exist elsewhere, Cloudflare's Workers bindings among them; what Golem adds is one model across files, network, secrets, agents, and tools, with attenuation, revocation, host-enforced middleware, and every denial in the oplog.

### What a card says

Each grant says whose resource it is, who holds the grant, a verb, and which part of the resource. Here is one, taken apart:

```
filesystem(?agent) @ ?agent : read : /data/**
```

- `filesystem(?agent)`: whose resource, here the holding agent's own filesystem.
- `@ ?agent`: the holder, the agent that holds the card.
- `read`: the verb.
- `/data/**`: which part, anything under `/data`.

In words: this agent may read anything under `/data` on its own disk. `?agent` stands for the agent holding the card, `?env` for the environment it is deployed in, and `?component` for the component it lives in, so one grant works for every agent of a type. Written out in full, names are paths: account, app, and environment, then component and agent type. The format is `resource @ holder : verb : pattern`, spaced exactly like that. More examples:

```
network() @ ?agent : connect : api.partner.com:443
secret(acme/shop/prod) @ acme/shop/prod/acme:cart-svc/CartAgent : reveal : cart.api-key
agent(?env/acme:payment-svc/PaymentAgent(*)) @ ?agent : invoke : charge
tool(?component/grep) @ ?agent : invoke : *
```

Filesystem grants match path segments (`*` is one segment, `**` any depth). Network grants match host and port. Tool grants match the tool, and can go down to the exact command, flags, and arguments.

A card lists its grants at two levels, and each level has *positive* grants and *negative* grants (vetoes). The **lower bound** is what the holder may do right now: a lower-bound negative overrides that card's own positives. The **upper bound** is a ceiling on what it may *ever* do, however many cards it is handed later: upper-bound grants hold across the whole wallet, and an upper-bound negative overrides every card. Think of the lower bound as the keys on your keyring, and the upper bound as the list of doors anyone may ever give you keys for.

### Where cards come from

![Where cards come from: the account root card leads to permission shares, environment defaults, and initial cards minted at deploy for each agent type; agents derive narrower cards at runtime, and each derivation can only narrow](/blog-images/whats-this-golem-cloud-thing-about-09-card-hierarchy.webp)

Every derivation can only narrow its parent. An agent can cut a smaller card for a helper, say read-only, one folder, thirty minutes, and pass it as a typed value in a call. It can never make one that opens more. Cards can **expire** (`"P30D"`), and **revoking** a card revokes every card derived from it: from the next invocation boundary onward, none of them authorizes anything new. To halt an agent mid-task as well, interrupt it, as we'll see later. You can't hand an agent more than you hold, either: initial cards must fall within the deployer's own authority.

### Opaque secrets and a free audit trail

A secret is a handle. An agent can hold it and pass it to a tool or service integration without ever seeing the plaintext. Revealing it requires an explicit `reveal` grant, and every reveal is audited. Golem does not inject secrets into ordinary HTTP requests, so sending a key in a header needs `reveal`. The agent's card decides whether the harness may reveal its model key, and each binding's `secretKeysRevealable` narrows what a tool may reveal: a binding that omits it passes on every secret the agent may reveal, so give tools that run generated code `secretKeysRevealable: []` and they get none. Once revealed, plaintext is ordinary data: a key placed in a request header is journaled with that request, so treat access to the oplog like access to the secret.

Every card event, from derivation, installation, and revocation to each denial, is journaled in the oplog. When an agent recovers, replay reproduces the same authorization decisions. The audit trail of who could do what, and when, comes for free.

### Defaults and least privilege

Out of the box, an agent type you deploy without further thought gets a **broad default card**: its own filesystem, any network destination, the environment's secrets (with the right to `reveal` them), and its stores, databases, agents, and tools. The default is a development convenience, wide so that your first deploy just works.

In production, every agent type gets its own **`initialCard`**, a few lines of YAML in `golem.yaml`. It **replaces** the default entirely, so the agent gets exactly what you list, plus access to the files the manifest seeds for it, and nothing else. That is how you reach least privilege. `golem card list --agent 'CodingAgent("acme", "u-17")'` shows what an agent actually holds.

### Allow `rm ./*`, never `rm -rf /`

Say a coding agent has an `fs` tool with an `rm` command (`fs rm [-r|--recursive] [-f|--force] <paths...>`). You want it to clean its workspace and never wipe anything else.

The rule that generalizes is a **filesystem** rule. Tools act with the calling agent's cards, so if the agent may only delete inside `/workspace`, then *no* `rm`, written any way, through any tool, can delete outside it. Add an exact-match veto on the tool for good measure:

```yaml
tools:
  fs:
    component: acme:fs-tools
agents:
  CodingAgent:
    tools:
      fs: { filesystemAccess: allowed }
    initialCard:
      lowerBound:
        positive:
          - 'tool(?component/fs) @ ?agent : invoke : *'
          - 'filesystem(?agent) @ ?agent : read : /workspace/**'
          - 'filesystem(?agent) @ ?agent : list : /workspace/**'
          - 'filesystem(?agent) @ ?agent : stat : /workspace/**'
          - 'filesystem(?agent) @ ?agent : delete : /workspace/*'
        negative:
          - 'tool(?component/fs) @ ?agent : invoke : rm "/" --recursive --force'
```

Now `fs rm ./notes.txt`, run in `/workspace`, passes both checks. `fs rm -rf /` is rejected before the tool even starts, by the exact negative grant; the canonical form spells flags out in full, so `-rf` and `--recursive --force` meet the same veto. And every variation, `rm -r /`, `rm /etc/passwd`, a `bash` script that calls `rm`, fails at the filesystem layer, because nothing outside `/workspace/*` is deletable.

Two precise points. Tool-argument grants are **exact matches** on the canonical form of the call; they don't do shell-style globbing. And a `bash` script is one argument, so cards cannot tell commands inside a script apart. That is why the path rule, not the argument rule, is the one that does the real work. Because an `initialCard` replaces the default, a real coding agent's card also grants `invoke` on its other tools, write access, and network egress.

### Prompt injection and the lethal trifecta

The *lethal trifecta* is private data, untrusted input, and a way to send data out. Put all three in one agent and a prompt injection can exfiltrate your data. Cards remove legs of the trifecta structurally. A card with no network egress, or with egress only to `api.partner.com:443`, leaves nowhere to phone home but the hosts you named. A tool bound with `secretKeysRevealable: []` never sees your API key, even when the harness that calls it uses that key. An upper-bound negative caps the agent even if it is later handed more cards.

To be precise about what this buys: **a card bounds the blast radius, not the behavior.** An agent can still misuse authority it legitimately holds. That is why irreversible effects deserve approval, and why approval gates belong in middleware.

The bottom line: what an agent *can* do is fixed by its cards, whatever it was talked into *wanting* to do. Prompt injection cannot grant authority.

## Artifacts: agents ship things people use

Agents that only answer API calls are half a product. People need a page to click, a report to download, a live view of work in progress. In Golem, these **artifacts** are served by the same runtime as the agent, from the same deployment, under the same authority, with the same audit trail.

### Code-first HTTP endpoints

Annotate an agent and its methods, and they become URLs:

```typescript
export const TaskAgent = defineAgent({
  name: "TaskAgent",
  id: { name: z.string() },
  http: http.mount("/task-agents/{name}"),
  methods: {
    createTask: method({ input: { request: CreateTaskRequest }, returns: Task, http: http.post("/tasks") }),
    getTasks: method({ input: {}, returns: z.array(Task), http: http.get("/tasks") }),
    completeTask: method({ input: { id: z.number() }, returns: Task.nullable(), http: http.post("/tasks/{id}/complete") }),
  },
});
```

`POST /task-agents/alice/tasks` reaches `TaskAgent("alice")`, creating it if needed. Path, query, and header parameters bind to inputs, return values map to sensible status codes, and Golem publishes an OpenAPI 3.1 document for the whole deployment at `/openapi.yaml`. Mounts and endpoints take `auth` and `cors` options.

### Routers, static files, and live files

When you want to own routing yourself, write an **HTTP router**: a small stateless agent that owns a mount point and handles standard Fetch `Request` and `Response` objects, so a Fetch-compatible framework can plug in.

```typescript
export const EchoRouter = defineHttpRouter("EchoRouter")
  .mount("/echo")
  .implement((request) => new Response(request.body));
```

A router can also serve **static files**, your HTML, CSS, and JavaScript, shipped as read-only files in the same deployment. Golem serves them straight from storage, with strong ETags, without starting an agent.

Any durable agent can publish **live files** from its own filesystem: the report it just wrote, the build it just produced. The URL always shows the agent's current contents, consistent with its state:

```typescript
http: http.mount("/files/{name}", { exposeFiles: [{ route: "/value", path: "/value.txt" }] }),
```

### Read-only methods and caching

Mark a method `readOnly: true` and the runtime enforces it: a write to a key-value store, blob store, or database, an outgoing HTTP request, or a call to another agent fails before anything persists. Leaving the agent's own fields unchanged is up to you. In return, Golem caches the result until the agent next writes. A cache hit skips the agent's queue and doesn't even load an evicted agent. Over HTTP, a read-only method on `GET` returns `Cache-Control` and an `ETag`, and a matching `If-None-Match` gets `304 Not Modified` without running any WebAssembly.

### Live streams

Any streaming method, like the ones shown earlier, mounted on an HTTP route is automatically exposed as a **Durable Streams** URL. Durable Streams is an open HTTP protocol for streams that a client can resume from any offset, a bit like server-sent events that survive reconnects. Golem implements the protocol's contracts, from catch-up reads and long-polling to idempotent appends and forks, and the unmodified reference client works against it. This is what token streaming to a browser should be: a tab that reloads picks up exactly where it left off.

### Login and domains

- **Front-end login.** Single-page apps authenticate users with the OAuth2 authorization-code flow with PKCE (`--login-mode authorization-code-pkce` on the security scheme). Golem issues a short-lived bearer token.
- **Who may call.** With `auth: true` on a mount and a security scheme attached to the agent in the deployment, a call without a valid login is turned away before the agent wakes. Inside, `this.getPrincipal()` tells the agent who is calling, so `CodingAgent("acme", "u-17")` can refuse everyone but user u-17.
- **Subdomains.** One line in the manifest gives a deployment a real address: `my-app.localhost:9006` on your laptop, `my-app.apps.golem.cloud` on Golem Cloud.

### One agent, every surface

The same agent is reachable from everywhere with no extra code:

![One agent, every surface: a browser with PKCE login, an MCP client, golem agent invoke, golem repl, the REST API, a bridge in your app, and other agents all reach CounterAgent("alice"), with one identity, one state, and one oplog](/blog-images/whats-this-golem-cloud-thing-about-10-every-surface.webp)

**MCP export** needs only the manifest: methods with parameters become MCP tools, parameterless methods become resources, and prompt hints become prompts. **`golem repl`** is a TypeScript REPL with typed clients for every agent in your app, whatever language they are written in. The **REST API** (`POST /v1/agents/invoke-agent`) is there for everything else.

### A page whose buttons call its agent

Here is the counter with a web page, end to end. The agent exposes three routes; a router serves the page at `/`.

```typescript
export const CounterAgent = defineAgent({
  name: "CounterAgent",
  id: { name: z.string() },
  http: http.mount("/counters/{name}"),
  methods: {
    value: method({ input: {}, returns: z.number(), readOnly: true, http: http.get("/value") }),
    increment: method({ input: {}, returns: z.number(), http: http.post("/increment") }),
    add: method({ input: { by: z.number() }, returns: z.number(), http: http.post("/add") }),
  },
});

export const CounterAgentImpl = CounterAgent.implement({
  init: () => ({ count: 0 }),
  methods: {
    value() { return this.count; },
    increment() { this.count += 1; return this.count; },
    add({ by }) { this.count += by; return this.count; },
  },
});

export const SiteRouter = defineHttpRouter("SiteRouter").mount("/").static("/", "/public/index.html").implement();
```

The page's buttons call `fetch("/counters/alice/add", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ by: 1 }) })`, and the page reads the current value from `/counters/alice/value`. Page and API share one origin, so no CORS setup is needed. The manifest ships the page and claims a subdomain:

```yaml
agents:
  SiteRouter:
    files:
      - { sourcePath: ./index.html, targetPath: /public/index.html, permissions: read-only }
httpApi:
  deployments:
    local:
      - subdomain: counter-demo          # http://counter-demo.localhost:9006/
        agents: { CounterAgent: {}, SiteRouter: {} }
mcp:
  deployments:
    local:
      - subdomain: counter-demo          # http://counter-demo.localhost:9007/mcp
        agents: { CounterAgent: {} }
```

One `golem deploy`, and the page loads from storage without waking anything, the buttons call a durable agent, the value read is cached with an ETag, and the same counter answers `golem agent invoke 'CounterAgent("alice")' add 5` from the terminal and a `CounterAgent` tool in your MCP client.

The agent ships its own UI from the same `golem deploy`: one deployment, one URL, one authority model, and one oplog, instead of a web app, a CDN bucket, an auth service, and glue.

## Living with agents

Shipping is the start, not the end. Agents outlive deploys, so you need ways to change, inspect, and test them in place. Most of these fall out of the oplog.

### Deploy, update, and rewind

`golem deploy` uploads your components and applies HTTP and MCP deployments. It doesn't start agents; they are created on first use. Existing agents stay on their current version until you move them:

```shell
golem deploy --yes --update-agents auto                 # deploy and update affected agents
golem agent update 'CounterAgent("c1")' auto --await    # or one agent at a time
```

An **automatic update** replays the agent's history against the new code. If the new code diverges from history (for example, it makes a different call at a step than the oplog recorded), the update fails safely and the agent keeps running on its old version. For breaking changes, a **manual update** saves state with the old code and loads it with the new.

**Revert** rewinds an agent: `golem agent revert <agent> --number-of-invocations 1` rolls its state back to before its last invocation. Effects already sent to the outside world stay sent; use sagas when those need undoing. `golem agent interrupt` and `resume` pause and continue a running agent, and `golem agent simulate-crash` proves recovery whenever you like.

An agent can also **fork**. `fork()` in the TypeScript SDK splits the agent at the current point in its execution: the child starts as a copy of the parent's history and files, both branches continue, and they can coordinate again through promises.

### The oplog is your audit trail

The oplog doubles as a complete audit trail, with nothing to configure: every HTTP call, agent call, tool call, file-tool command, card event, and middleware decision, in order. Search it with Lucene syntax:

```shell
golem agent oplog 'CodingAgent("acme", "u-17")' --query 'external-tool'   # tool calls made through golem ssh or golem tool invoke
golem agent oplog 'OrderAgent("order-42")' --query 'start AND request.product-id:123'
golem agent stream 'CodingAgent("acme", "u-17")'          # live stdout, stderr, and logs
golem agent files 'CodingAgent("acme", "u-17")' /workspace
```

### OpenTelemetry

Add the built-in `golem-otlp-exporter` plugin to a component in `golem.yaml`, point it at your collector, and Golem exports traces (with W3C trace context propagated in and out), logs, and per-agent metrics such as invocation count, duration, CPU consumed (fuel), and memory. Custom spans are available in every SDK, and a span survives across invocations and crashes because its lifecycle is part of the durable execution.

### What Golem doesn't do

Golem does not make a model smarter, and it does not replace evals. It makes every run inspectable in the oplog, revertable, and forkable, so a production failure is something you study, not guess at.

## Scale and cost

Everything so far has been about one agent. Here is what happens with millions of them.

### Waiting costs nothing

Agents spend most of their lives waiting: for a model, a human, a webhook, tomorrow's schedule. A suspended agent costs only the storage for its oplog, files, and snapshots. On Golem Cloud, memory is metered as allocated WebAssembly memory over time, so a suspended agent accrues none.

### Two kinds of limits

An agent looping at midnight should hit a limit, not your credit card. Golem has two kinds:

| | User-defined quotas | Plan limits |
|---|---|---|
| Purpose | Hard budget caps on *your* spend (LLM tokens, API calls, connections), which agent code draws on before each call | Protect the platform |
| Defined by | You, in `resourceDefaults` or `golem resource` | The operator's plan |
| Shapes | Rate, capacity, concurrency | Memory and disk per agent (1 GB each by default in open source), monthly compute, counts |
| When exceeded | Your choice: `reject`, `throttle` (the agent suspends until capacity returns), or `terminate` | Creation rejected, growth capped, or durable agents suspended |

```shell
golem resource update api-calls \
  --limit '{"type":"Rate","value":200,"period":"minute","max":2000}' --environment prod
```

Quotas are enforced locally on each node with leases from the shard manager, so there is no coordination per request. In code, the harness reserves an estimate from a token quota before each model call and commits what it actually used, so a runaway loop stops at the cap; an agent can also split its budget among child agents. `golem account usage show` reports compute, memory, and storage use by month.

### Sharding

Agents are partitioned into shards spread across executor nodes. A stateless gateway finds the node that owns an agent and routes the call. When nodes join or leave, shards move, and agents on a failed node recover from their oplogs on healthy ones. Only one node owns a shard at a time, and that ownership is enforced, not assumed.

You will never see this in a demo. You would notice the day it was missing. You pay for work, not for waiting, and spreading millions of agents across machines is already solved.

## How Golem compares

With the whole runtime in view, here is Golem next to the tools you already know. Each row describes a category at its best, then what Golem changes.

| | At its best | What Golem changes |
|---|---|---|
| **Durable execution engines** (Temporal, Restate, Inngest, DBOS, Cloudflare Workflows, Vercel Workflow) | Steps journaled and replayed after failure, durable timers, and human-in-the-loop waits. Mature, proven at scale, broad language support (Python included), many framework integrations, usually on your own compute | No steps to mark and no determinism rules in your code; updates to live agents are checked against history. The unit is a long-lived agent with memory, files, and cards, updated in place, with tools, artifacts, and authority in the same runtime |
| **Stateful actor platforms** (Orleans, Akka, Rivet, Cloudflare Durable Objects with the Agents SDK) | Agents addressed by key, single-threaded, asleep when idle, with persisted state. Proven at scale; Rivet and Durable Objects add realtime connections and embedded SQLite, and the Agents SDK adds checkpointed fibers with a recovery hook for long tasks | Recovery resumes mid-method with no checkpoint call or recovery hook, agent-to-agent and tool calls execute exactly once, and durable state and durable steps are one primitive |
| **Agent frameworks** (LangGraph, OpenAI Agents SDK, Vercel AI SDK, Mastra) | The harness: model adapters, tool calling, memory, evals, and UI hooks. Durability usually means a checkpointer or a separate engine, and sandboxes come from a separate provider or built-in sandbox features | The runtime underneath: every effect inside a step is journaled, so nothing restarts from the top and tools need no idempotency code. The Vercel AI SDK and TanStack AI run inside Golem agents, durable with no checkpointer or step directive |
| **Managed agent platforms** (Anthropic's managed agents, AWS Bedrock AgentCore, Google's agent runtime) | A hosted harness or runtime with sessions, memory, sandboxes, identity, and policy, operated for you in one cloud | Open source (BUSL-1.1 → Apache-2.0) and self-hostable. Agents can live for months, every effect is journaled, and one card model governs agents, tools, and artifacts |
| **Sandboxes** (E2B, Modal, Daytona, Vercel Sandbox) | Full Linux per session behind a microVM or hardened container: any binary, Python, browsers, and GPUs on some; snapshots, egress policies, and secret brokering | Each tool call gets its own instance that can work on the agent's disk, and execution, not just disk, survives a crash. Native toolchains stay in a sandbox you drive as a tool |
| **Containers, VMs, and serverless functions** | A process sandbox with its own userland or kernel, or stateless handlers with timeouts; state lives in an external database | Agents keep memory and files, run for seconds or months, suspend to zero while waiting, and are addressed by name |

Golem isn't another layer on top of these. It is the layer underneath your harness, and it absorbs most of what you would otherwise assemble from them.

## Your language

Nothing so far has been specific to TypeScript. Golem 1.6 ships six SDKs across five languages (Effect is a TypeScript SDK built on the Effect library), and they share one agent model, one schema system, and one component format.

| | TypeScript 2.0 | Effect 2.0 | Rust 3.0 | Scala 2.0 | MoonBit 0.6 | Go 0.1.0 (new) |
|---|---|---|---|---|---|---|
| Define an agent | `defineAgent` | `defineAgent` | `#[agent_definition]` | `@agentDefinition` | `#derive.agent` | `golem.DefineAgent` |
| Embedded SQLite | ✓ | ✓ | ✓ | — | — | — |

All six support durable, ephemeral, and phantom agents, agent calls, tools and middleware, HTTP endpoints and routers, streaming and Durable Streams, read-only methods, promises and webhooks, retry policies, atomic regions and sagas, config, secrets, cards and quotas, reflection, snapshots, bridges, and PostgreSQL and MySQL.

TypeScript works with the Standard Schema libraries Zod, Valibot, ArkType, and Effect Schema. The Go SDK is new in 1.6 and defines agents with generics.

There is no Python SDK in Golem 1.6. Python programs call agents over the REST API, MCP, or HTTP endpoints, and Python code an agent needs runs in a VM driven as a tool, the top rung of the execution ladder.

The counter you've seen so far is TypeScript. Here is the counter in Rust, from the default template:

```rust
#[agent_definition(mount = "/counters/{name}")]
pub trait CounterAgent {
    fn new(name: String) -> Self;          // constructor parameters = identity

    #[endpoint(post = "/increment")]
    fn increment(&mut self) -> u32;
}

struct CounterImpl { _name: String, count: u32 }

#[agent_implementation]
impl CounterAgent for CounterImpl {
    fn new(name: String) -> Self { Self { _name: name, count: 0 } }
    fn increment(&mut self) -> u32 { self.count += 1; self.count }
}
```

And in Go:

```go
// package counter: the definition, which other agents import to call it
type ID struct{ Name string }

var Agent = golem.DefineAgent[ID](golem.Spec{Name: "CounterAgent"})
var Increment = Agent.Method[golem.Unit, int64]("increment")

// package impl: the implementation
type state struct{ count int64 }

var agent = counter.Agent.Implement(func(counter.ID) *state { return &state{} })

func init() {
	agent.Handle(counter.Increment, func(ctx *golem.Context[state], _ golem.Unit) int64 {
		ctx.State.count++
		return ctx.State.count
	})
}
```

Fields are memory, constructors are identity, and Golem keeps both. A TypeScript agent calls the Rust one, or the Go one, through a generated typed client. So pick a language for its ecosystem, not its feature list, and let polyglot teams share agents instead of rewriting them.

## Putting it together: a coding agent, entirely in Golem

The coding agents with the most reach are the ones business users never notice: a support bot that writes a script to reconcile an account, or an analyst's assistant that pulls a report together from three systems. We built a TypeScript coding agent that runs entirely inside Golem 1.6, `npm`, `npx`, and `tsc` included, and we will open source it. Here is one request, end to end, and every concept in this post shows up.

A user types: *"Reconcile account 4471 against last month's invoices and give me a report."*

![A coding agent entirely in Golem: the browser logs in through a static SiteRouter and posts to CodingAgent("acme", "u-17"), whose harness loop calls the model provider over egress named in its card, whose tools write files, run npm install and tsc, and commit with git on its own durable /workspace, whose oplog records every model call, tool call, card check, and middleware decision, and whose progress streams back to the browser over a Durable Stream](/blog-images/whats-this-golem-cloud-thing-about-11-coding-agent.webp)

1. **The request arrives** at a code-first route, from a front-end served by a router with PKCE login. `CodingAgent("acme", "u-17")` is created on first use.
2. **The harness**, the loop that drives the model and runs its tool calls, is ordinary TypeScript calling the model provider over durable HTTP. The agent's card names only the hosts it needs, the model provider and the npm registry, as egress. The harness holds `reveal` for the model key; the tools that run model-written code get none.
3. **The model writes code** with `write-file` and `edit-file` into the agent's own filesystem. `path-policy` middleware confines those tools to the workspace, and the host enforces it on every call.
4. **It builds.** `npm install`, `tsc`, `node`: each a tool call in a fresh, isolated WebAssembly instance, working on the agent's disk because the binding allows it. A delete grant limited to `/workspace` bounds every tool, `bash` included.
5. **It streams** its progress to the browser through a Durable Stream. Reload the tab and it picks up where it left off.
6. **The node dies** halfway through `npm install`. The agent's shard moves to a healthy node, and the agent recovers from its oplog with its files intact. The interrupted `npm install` is finished as that same tool call, not started again as a new one, and completed tool calls are not performed again: every tool call executes exactly once. The session carries on.
7. **It waits for sign-off.** If a manager must approve the report, the agent awaits a promise and suspends to zero until the click arrives.
8. **It leaves a record.** The oplog lists every model call, command, file-tool edit, tool call, card check, and middleware decision. `git` holds every change on a branch you can inspect. `golem ssh` opens a prompt in the sandbox, and `golem agent revert` rolls the agent's state back by whole invocations.

### Why not a separate VM sandbox?

The prevailing view in agent architecture is to decouple the brain from the hands: the harness in one place, model-written code in a separate sandbox, credentials away from both. The goals are right: the harness must not die with the sandbox, credentials stay away from generated code, and sandboxes are cattle, not pets. Golem meets each of them, and a few more, by construction, without a second system:

| The goal | On Golem |
|---|---|
| Model-written code runs apart from the harness | Every tool call is its own sandbox, a fresh WebAssembly instance that never sees the harness's memory and shares the agent's disk only when its binding allows. Keep what the harness must trust in memory, or on paths where the agent's card grants no write or delete, a rule every tool inherits; bind a tool with `filesystemAccess: denied` when it needs no disk, and use `path-policy` to narrow the file tools further |
| Credentials stay away from generated code | `reveal` narrowed per binding to where it is needed: the harness can use its model key while the tools that run generated code never see it; the harness is code you wrote, and the model sees the key only if your code puts it in a prompt |
| The harness survives a crashed sandbox, and sandboxes are cattle | Execution, not just disk, survives: any agent is rebuilt on any node from its oplog, and an interrupted tool call is finished once, as that same call |
| Harness and workspace stay consistent | One journaled unit holds both; there is no second system to sync |
| Guardrails out of the agent's reach | Host-enforced middleware; cards narrowed per agent, tool binding, and derived card |

A VM kept alongside has costs of its own. Its execution is not journaled with the agent's, so even with a durable harness it can lose work in flight and drift away from the agent driving it, and it adds cost and latency to work that doesn't need it. Native toolchains that cannot run in WebAssembly, such as containers, full browsers, or Python with native libraries, take the ladder's top rung: the durable harness stays in Golem and drives a VM as a tool.

## Where to run it and how to start

You've seen the whole picture. Here is how to run it.

### Where it runs

- **On your laptop.** The `golem` binary includes a complete local Golem: `golem server run` starts the executor, registry, shard manager, and gateway in one process, with the API on port 9881, HTTP endpoints on 9006, and MCP on 9007. Stop and restart it, and your agents are still there.
- **Golem Cloud.** The hosted service is in Developer Preview today: free, for building and trying things out, not yet for production. Paid plans with usage-based pricing launch in the coming weeks. Deploy with `golem deploy --cloud`, and your apps appear at `<name>.apps.golem.cloud`.
- **Self-hosted.** Run the published Docker images with Docker Compose, or on Kubernetes with your own manifests. Storage backends include PostgreSQL, Redis, SQLite, S3, and etcd.

Golem is open source (BUSL-1.1 → Apache-2.0). Production use is allowed, including commercial applications built on Golem; the one exclusion is offering Golem itself to third parties as a hosted developer platform. Each version converts to Apache-2.0 four years after release.

### Five minutes to a durable agent

```shell
# install: download the golem binary for your platform (learn.golem.cloud, Quickstart) and put it on your PATH
# the TypeScript template also needs Node.js 24.11+ with npm 11.6.2+

# terminal 1
golem server run

# terminal 2
golem new --template ts --component-name example:counter --yes agent-examples
cd agent-examples
golem build
golem deploy
golem agent invoke 'CounterAgent("my-counter")' increment   # 1
golem agent invoke 'CounterAgent("my-counter")' increment   # 2
golem agent simulate-crash 'CounterAgent("my-counter")'     # kill it
golem agent invoke 'CounterAgent("my-counter")' increment   # 3: it came back

# terminal 1: stop the server with Ctrl-C, then start it again
golem server run

# terminal 2
golem agent invoke 'CounterAgent("my-counter")' increment   # 4: the state survived the restart
golem repl     # >>> const c = await CounterAgent.get("my-counter"); await c.increment()
```

Swap `--template ts` for `effect`, `rust`, `scala`, `moonbit`, or `go`, after installing that language's toolchain (learn.golem.cloud, Setup); the Effect template, which has the same Node.js requirement, names its agent `Counter`, so call `'Counter("my-counter")'` there.

### Let your coding agent build it

Every project created by `golem new` includes an `AGENTS.md` and a set of skills, in `.agents/skills/` and `.claude/skills/`, covering everything from defining tools to passing permission cards and updating running agents. Before you have a project, `npx skills add golemcloud/skills` installs bootstrap skills for Amp, Cursor, or Claude Code. Every non-interactive CLI command can emit JSON with `--format json`, against published schemas, so a coding agent can drive the CLI without scraping text.

## Where to go next

That's the whole idea. You write the agent as ordinary code, and Golem makes it durable, isolated, governed, and reachable, along with the tools it uses and the things it ships.

Start with the five-minute counter, crash it, and watch it come back. Then create an app with `--template ts/human-in-the-loop`, restart the server while its workflow waits for approval, and approve it afterward through the template's `HumanAgent`. Then ask your coding agent to add a tool (the bundled skills know how), and give its agent an `initialCard` like the one above. To feel the capstone today, bind `bash`, `npm`, and `tsc` to an agent with `filesystemAccess: allowed`, `golem ssh` into it, and run `npm install csv-parse` there.

Go deeper at [learn.golem.cloud](https://learn.golem.cloud). The agent at the start of this post died at step 7 and paid for steps 1 to 6 again. On Golem, it resumes at step 7.
