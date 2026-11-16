# Golem homepage — optional visitor details

Draft 2 · Optional disclosures, separate from the default reading.

## 1. Hero

**What does zero infrastructure mean?**

An app can keep its state, files and SQLite database on Golem, serve its interface and API, and wait without a running VM. You do not provision a separate database server or sandbox service for each app.

## 2. Two primitives

**Actor lifetime and concurrency**

The same actor ID reaches the same actor across method calls and host changes. Its identity stays stable while Golem preserves its execution and private state. Actors are single-threaded; invocations can interleave at await points.

**Function durability and state**

A durable function invocation resumes its own execution after interruption. It retains no private instance state for later independent invocations. A bound tool can receive access to the caller's files without access to its memory.

**Function identity and middleware**

Each function has an identity, permissions, middleware and billing of its own. Independent invocations run in separate VMs; the function's identity is distinct from each invocation's VM. Use the same foundation for HTTP request handlers, tools and background work.

Middleware adds policy checks, observability or human approval around a function without changing its implementation.

## 3. The VM

**View: Wait and resume**

Your actor waits for approval. Its running VM unloads, while its execution remains in storage. When approval arrives, it resumes where it left off. Waiting uses no CPU or RAM.

**View: Recover on another node**

Golem records external interactions off the node. After interruption, replay reconstructs the VM and reuses recorded results, including completed model replies. You do not write checkpoint code or operate a separate recovery workflow.

**The whole VM, beyond saved data**

Golem reconstructs the program's memory, call stack, execution position, environment and filesystem. Ordinary code continues inside the interrupted function. SQLite, CozoDB and compatible embedded databases use durable files in that VM; you do not rewrite the application around a particular storage API to make its private state persistent.

**Authority is enforced by the runtime**

The policy boundary surrounds your application code: actors, functions and their tools. Permission cards are unforgeable WebAssembly component handles. Compose grants, set required and maximum authority, and explicitly authorize delegation or elevation. Knowing a resource ID never grants access to it. Golem enforces the resulting authority even when the model, generated code or a dependency asks for more.

**What does the asterisk mean?**

_Effects inside durable Golem execution, including file changes, take effect once. Completed external requests replay their recorded results. An interrupted external request may be resent with the same idempotency key; preventing duplicate effects requires the receiving service to honor that key. Database transaction details are covered in the durability guide. Functions declared idempotent may be rerun._

## 4. What you build

**An app can bring its own data**

Actor memory and files are already durable. Add a local SQLite database for structured queries, or use CozoDB for relational and graph data. Connect external databases when other systems need shared data.

**Keep the data as the app changes**

An actor-backed application can gain views, integrations and behavior while retaining its data. Code and schema updates use Golem's supported update and migration mechanisms.

## 5. Shared capabilities

**Data and memory**

Use SQLite and CozoDB from every SDK, through guest libraries or standard tools. Keep an agent's working knowledge alongside its durable code and files.

**Publish and reuse**

Install packages of actors and functions, configure their bindings and expose them through HTTP, MCP or A2A. Publish generated content with controlled access.

## 6. Agentic highlight

**Build and deploy from inside Golem**

Running code uses the Golem deployment tool under its own granted authority, without receiving a person's account credentials. It can build supported projects, deploy prebuilt components, inspect results and manage the software it is permitted to operate.

Complete coding agents can run inside Golem in TypeScript and MoonBit, including their file-editing, build and deployment tools. Other SDKs can run actors and functions and deploy prebuilt components through authorized tools; this does not imply a complete in-VM build toolchain for every language.

**Models, tools and secrets**

Use Golem's model SDK or supported provider libraries. Connect remote MCP tools and expose software through MCP or A2A. Delegate, expire and revoke permissions, and use secret handles to authenticate without exposing the secret value to the agent.

## 7. Languages and libraries

**Example: wait without a running VM**

The excerpt is ordinary code within a durable actor method. `sleep` is the Node.js timers/promises timer; `writeFile` is the filesystem operation. `summarize` and `render` are application functions. The same execution also recovers after infrastructure interruption; the runtime handles that recovery.

**Example: keep a database with the actor**

Use language-native SQLite libraries where supported, or invoke the standard SQLite tool against the actor's durable files. CozoDB supports relational and graph queries in the same local-data model.

**Example: call from another application**

The bridge generator produces a typed client for your actors and functions. Configure the client for the deployment, then call it from an existing frontend, service or application in its normal language. A bridge is an invocation interface; it does not move the calling application's execution into Golem.

**Package compatibility**

The runtime-support pages distinguish supported, adapted and unavailable APIs. Pure-Python packages work within supported runtime facilities; native modules use WebAssembly ports. Node APIs such as process execution map to Golem tools where supported.

## 8. Deployment and operations

**What runs where?**

The application model is the same locally, self-hosted and managed. Golem supplies the execution platform. Actors and tools with deployment authority can create more software on the Golem server they run on.

## 9. Getting started

**Explore the guarantees**

After the first app works, ask your agent to add a durable wait and show the actor unloading; interrupt the platform and resume the work; then grant a tool access to one sample file and demonstrate a denied access elsewhere.

**Explore apps deploying apps**

Ask your agent to build a small Golem builder actor with permission to deploy into a local project. Have it deploy the spending app from inside Golem, then show the resulting app and its separate identity. Use the documented tool and complete supported build pipeline.
