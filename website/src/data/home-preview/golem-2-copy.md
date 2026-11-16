# Golem homepage — copy-only draft

Draft 2 · Default-visible reading. [Full content, optional details and visual direction](golem-2-homepage-draft.md).

## 1. Hero

**Eyebrow**

Open source app runtime

**Heading**

The cloud for the agent era

**Introduction**

Build agents, tools and complete apps—with zero infrastructure to assemble per app.

**Execution foundation**

Golem runs your code in lightweight, durable VMs with persistent memory, call stacks and filesystems. The runtime enforces permissions your code cannot bypass. Waiting VMs suspend to zero CPU and RAM.

**Deployment choices**

Local · Self-hosted · Golem Cloud

**Actions**

- Start building →
- Explore the platform ↓

**Proof strip**

Built on Golem

Ziverge · Golem Social · WarpMind · Seeta AI Assistant · JournAI · Johnethel LMS

## 2. Two primitives

**Eyebrow**

The programming model

**Heading**

Two primitives. A foundation for distributed computing.

**Introduction**

Actors keep state. Functions serve as tools and request handlers. Compose both in Golem's durable, permissioned VMs to build complete distributed applications.

**Actors**

Stateful across calls

One stable identity, with private memory, files and databases across calls. Code keeps its place through waits and restarts. Messages to the same actor ID reach the same actor.

**Functions**

Stateless between calls

A fresh VM for each call, durable by default through waits and interruptions. Private instance state ends with the call. Each function has its own identity, permissions and middleware.

**Actor examples**

- Agent
- App
- Shopping cart

**Function examples**

- Tool
- HTTP handler
- Background job

**Interfaces**

- HTTP
- MCP
- A2A
- CLI

**Link**

Explore actors and functions →

## 3. The VM

**Eyebrow**

The execution foundation

**Heading**

One lightweight VM. Two fundamental guarantees.

**Introduction**

The whole WebAssembly VM is durable: memory, call stack, execution position, environment and files. Keep SQLite, CozoDB or another compatible embedded database inside it.

**Metrics**

| Figure            | Label                    |
| ----------------- | ------------------------ |
| 10,000+           | Actors per node          |
| As little as 2 ms | VM startup               |
| 1 MB              | Minimum memory per actor |
| Zero CPU / RAM    | While suspended          |

**Transparent durability**

The program survives, not just saved data. Golem records each I/O operation automatically and resumes inside the same function after a wait or failure. No checkpoint code. No workflow steps to define.

Transactional execution makes durable code take effect once, even when interrupted midway.\*

**Durability payoff**

Keep your place. Stop paying for idle compute.

**Runtime-enforced permissions**

The runtime enforces security, not your application code. Golem checks file, network, secret, tool and deployment access for actors and functions. Generated code and dependencies cannot bypass the policy.

**Security payoff**

Let AI write the code. Golem enforces your policy.

**Links**

- How durable execution works →
- How permissions work →

## 4. What you build

**Heading**

Build for one person. Or everyone.

**Introduction**

Build an agent, a personal app or a distributed backend. Golem brings execution, private data, APIs and serving together on one platform.

**Agents and tools**

Give agents their own workspaces and access to isolated tools. Let them work, wait for people and resume their execution, under permissions that their code cannot bypass.

Explore agents and tools →

**Disposable apps**

Ask for an app. Keep shaping it.

An agent can build and deploy the interface, backend and working data for a particular need. Add capabilities through further requests. Use it once or keep evolving it for years.

The app keeps its state, files and SQLite database inside its durable VM, with its interface and API served on Golem. No separate database server or per-app sandbox service to provision. When inactive, its actor suspends to zero CPU and RAM.

Explore disposable apps →

**Distributed systems**

Model accounts, orders and devices as actors. Use functions for independent work. Coordinate them with typed calls and transactional execution, with permissions enforced throughout.

Explore distributed systems →

## 5. Shared capabilities

**Heading**

The platform around your code

**Give work a budget**

Set resource limits and quotas. Account for usage and choose what happens when a limit is reached.

**See what happened**

Inspect calls, results and permission decisions in the journal. Export OpenTelemetry traces and metrics.

**Coordinate work**

Make typed calls across languages. Wait for webhooks, schedules and people without holding a VM in memory.

**Operate applications**

Inspect execution, deploy updates and manage long-lived actors. Recover faster with execution snapshots.

Explore platform capabilities →

## 6. Agentic highlight

**Eyebrow**

Built for agents that act

**Heading**

Deploy apps. Let your code deploy more.

**Introduction**

The coding agent can live here too. TypeScript and MoonBit agents can edit, build and deploy entirely inside Golem, using the same runtime and permission model as the apps they create.

**A complete app, created by an agent**

An app brings its interface, API, actor and embedded database. Golem hosts the whole application, so the agent has no separate infrastructure stack to assemble.

**The same power at every level**

That app can create another app, actor or function when authorized. Every new instance gets its own identity and runtime-enforced permissions on the same Golem foundation.

**Authority stays outside the model**

Let an agent deploy in one project and deny deployment elsewhere. Golem enforces the boundary around the agent and every tool it uses; the model cannot grant itself more.

**Work when there is work**

Actors can wait for people, events or the next user. Golem unloads them to zero CPU and memory, then resumes the same execution with working state and files intact.

Build an agent on Golem →

## 7. Languages and libraries

**Heading**

Your language. The same foundation.

**Introduction**

Choose from seven SDKs. Use familiar libraries, keep embedded databases with your code, and call actors and functions across languages through typed interfaces.

**Language selector**

TypeScript · Effect · Rust · Scala · MoonBit · Go · Python

**Example — inside an actor method**

<!-- prettier-ignore -->
```typescript
const summary = summarize(transactions)
// The VM unloads during the wait, using no CPU or RAM:
await sleep(7 * 24 * 60 * 60_000)
// Resumes with summary, memory, stack and files restored:
await writeFile('/reports/monthly.html', render(summary))
```

**JavaScript and Python ecosystems**

Use a Node.js-compatible runtime with supported npm packages, or Python with pip and supported native-module ports. SQLite and CozoDB are accessible from every SDK.

**Connect existing applications**

Connect your existing applications to Golem with generated, typed bridge clients. Use these clients in your existing code to call Golem actors and run Golem functions.

**Links**

- Find your SDK →
- Explore packages and runtime support →

## 8. Deployment and operations

**Heading**

Run Golem wherever you need it

**Introduction**

Your whole application runs on Golem. Choose local development, infrastructure you control or the managed Golem Cloud.

**Local**

One executable runs the platform on your laptop. Create an app and see it wait, resume and enforce permissions.

**Your infrastructure**

Deploy the open source platform with Docker or Kubernetes. Run your applications on infrastructure you control.

**Golem Cloud**

Run your applications on the managed Golem platform. Let Golem operate the infrastructure while you build and deploy.

**Actions**

- Run locally →
- Self-host Golem →
- Explore Golem Cloud →

**Customer proof**

“Tried Temporal, Dapr, Restate, and Fermyon Spin. Golem gives me what I'd given up on finding: code-level customizability with no-code-level operational simplicity.”

Samuel Iyeh · AI Software Engineer · Building Johnethel LMS

## 9. Getting started

**Heading**

Give your agent its next project

**Introduction**

Copy this into your coding agent. Start with a working app, then keep shaping it.

**Copyable instruction**

> Using the official Golem documentation at https://learn.golem.cloud, build and run a small spending tracker locally. Give it a browser interface, a durable actor and its own SQLite database, with sample transactions. Show me the working app first. Then help me explore waiting without compute, recovering after interruption, restricting a tool's access, and adding category budgets while keeping the data. Use the documented SDK and update mechanisms.

**Actions**

- Copy instructions
- Follow the quickstart →
- View on GitHub →
