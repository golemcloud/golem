---
name: golem-call-another-agent-ts
description: "Calling another agent and awaiting the result in a TypeScript Golem project. Use when the user asks about agent-to-agent RPC, calling remote agents, or inter-component communication."
---

# Calling Another Agent (TypeScript)

There are two typed RPC APIs. Use a definition's `.client` when both agents are defined in the
same component. Use a generated guest client when the target is in another
component (including a component written in another language).

## Same Component: Definition Client

The `.get` examples below target durable agents. Ephemeral definitions have no `.get`:
use `const client = EphemeralAgent.client.newPhantom(id)` directly (not `.client` on
the returned value). Awaited ephemeral calls return `{ metadata, value }`; for a
declared result inspect `response.value.isErr()`, not `response.isErr()`.

Use the `.client` attached to the agent's **definition** (the value returned by
`defineAgent`), then call `.get` with the target agent's **id record**:

```typescript
import { Counter } from './counter-agent.js';

// Get a handle to a specific instance by its id record.
const c1 = Counter.client.get({ name: 'my-counter' });
```

The id argument is the record declared in the target agent's `id: { … }` (for a
`Counter` with `id: { name: z.string() }`, that is `{ name: 'my-counter' }`).
This does **not** create the agent — the agent is created implicitly on its first
invocation. If it already exists, you get a handle to the existing instance.

Call a method and wait for the result. Method inputs are passed as the declared
input record (methods with an empty `input: {}` take no argument):

```typescript
const count = await c1.increment();          // input: {}
const next = await c1.add({ by: 5 });        // input: { by: z.number() }
```

The calling agent **blocks** until the target agent processes the request and
returns. This is the standard RPC pattern. Invocation or transport failures can
throw a `RemoteCallError`. A declared `s.result(ok, err)` is instead decoded as an
ordinary `Result` return value, including `Result.err`; inspect it with `.isErr()`
and `.unwrapErr()` rather than catching it as an RPC failure.

## Phantom Agents

For durable agents, call `Def.client.getPhantom(id, phantomId)` to address a specific phantom
instance that shares the same id record. To create a fresh phantom, call
`Def.client.newPhantom(id)`; the returned details contain the typed client, full
`agentId`, and generated `phantomId`, which can be saved and reused. See the
`golem-multi-instance-agent-ts` skill.

## Different Component: Generated Guest Client

Declare the dependency in the application-level `golem.yaml`. The CLI infers the
consumer language from its `ts` template and generates the client before
compiling the dependent component:

```yaml
components:
  my-app:counter:
    dir: counter
    templates: rust
  my-app:consumer:
    dir: consumer
    templates: ts
    dependencies:
      agents:
        - my-app:counter/CounterAgent
```

The generated package is
`golem-temp/bridge-sdk/ts/internal/counter-agent-guest-client`. From the
`consumer` component directory, wire its source into the existing
`tsconfig.json` (keep the component's own `src/**/*.ts` include):

```json
{
  "compilerOptions": {
    "paths": {
      "counter-agent-guest-client": [
        "../golem-temp/bridge-sdk/ts/internal/counter-agent-guest-client/counter-agent-guest-client.ts"
      ]
    }
  },
  "include": [
    "src/**/*.ts",
    "../golem-temp/bridge-sdk/ts/internal/counter-agent-guest-client/*.ts"
  ]
}
```

Import that alias. Generated constructors flatten the target's id fields into
parameters rather than taking an id record. This example targets a durable agent:

```typescript
import { CounterAgent } from 'counter-agent-guest-client';

const counter = CounterAgent.get('my-counter');
const count = await counter.increment();

counter.increment.trigger();
counter.increment.schedule(runAt);
const cancellation = counter.increment.scheduleCancelable(runAt);
```

Every stream-free generated method supports an awaited call plus `.trigger`,
`.schedule`, and `.scheduleCancelable`. A method whose input or output schema
recursively contains a stream is awaited only; use `AgentStream` values as
described by the `golem-streaming-agent-ts` guide.

Optional parameters at the end of a constructor or method parameter list can be
omitted; an optional parameter followed by a required one must be passed
explicitly, as `undefined` when absent. A `result<ok, err>` value is the SDK
`Result` in both directions: check `value.tag === 'ok'` (or `value.isOk()`) and
read `value.val`, and pass arguments as `Result.ok(...)` / `Result.err(...)`.

No npm install, package path
dependency, manual bridge-generation command, or REST client `configure()` call
is needed: `golem build` regenerates the bridge before the consumer compiles.

## Avoiding Deadlocks

**Never create RPC cycles** where A awaits B and B awaits A — this deadlocks both
agents. Use `.trigger()` (fire-and-forget) to break cycles. See the
`golem-fire-and-forget-ts` skill.
