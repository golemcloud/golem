---
name: golem-call-from-external-effect
description: "Calling Effect-based Golem agents from external Node.js applications with generated TypeScript bridge clients and Effect v4. Use for standalone Effect CLIs, servers, or scripts that invoke deployed agents from outside Golem."
---

# Calling Effect Agents from External Applications

Use Golem's generated **external TypeScript bridge** for a standalone Effect/Node.js application.
An Effect component publishes TypeScript-shaped agent metadata, so its generated bridge classes,
constructor arguments, method names, values, and CLI references use TypeScript syntax.

The `client` attached to an `@golemcloud/effect-golem` agent spec is not an external network
client. It uses the Golem `golem:agent/host` RPC binding and works only inside a deployed Golem
component. Do not import that client, `HostLive`, or any other Effect Golem host service into a
standalone Node.js process.

## Steps

1. Ensure the Effect agent has the required TypeScript name and method contract, then build it.
2. Configure a `ts` bridge in `golem.yaml`; there is no separate `effect` bridge target.
3. Run `golem build` to regenerate the bridge from the built agent metadata.
4. Deploy the built application so the external client can reach the current agent definition.
5. Install and build the generated npm package.
6. Add the generated package and the same Effect v4 version used by the component to a standalone
   Node.js project.
7. Configure the generated client, wrap its Promise operations with `Effect.tryPromise`, and run
   the Effect program with `Effect.runPromise`.

## Generate the TypeScript Bridge

Add or extend the top-level `bridge` section in `golem.yaml`:

```yaml
bridge:
  ts:
    agents:
      - CounterAgent
    outputDir: ./bridge-sdk/ts/counter-agent-client
```

`agents` accepts `"*"` or a list containing agent type names and component names
(`namespace:name`). Preserve any existing bridge languages and selected agents.

For this Golem manifest version:

- use `bridge.ts.agents`, not `bridge.effect`;
- do not add an `external:` level under `ts`;
- a custom `outputDir` is the generated package directory itself, not a parent directory for all
  generated clients;
- without `outputDir`, `CounterAgent` is generated under
  `golem-temp/bridge-sdk/ts/counter-agent-client/`.

Generate the bridge as part of the normal build:

```shell
golem build
```

The build first compiles the Effect component, reads its final agent metadata, and then generates
the selected bridge packages. Re-run it whenever the constructor, methods, or schemas change.
Prefer this manifest-driven flow over the low-level `golem generate-bridge` command.

Deploy the same build before running an external client, especially after renaming an agent or
changing its contract:

```shell
golem deploy --yes
```

Build the generated package before consuming it:

```shell
cd bridge-sdk/ts/counter-agent-client
npm install
npm run build
```

The generated directory is a self-contained ESM package. For `CounterAgent`, its package name is
`counter-agent-client`, and its main module exports the `CounterAgent` class and `configure`
function. Inspect the generated `.d.ts` file when exact constructor or method types are needed;
do not guess them from the component source.

## Set Up the Standalone Effect Application

Create the external application outside the Golem component source. Add the generated package as
a file dependency and install Effect v4. Keep the `effect` version exactly aligned with the root
Effect component's `package.json` (the pinned SDK uses `4.0.0-beta.98`):

```json
{
  "name": "external-client",
  "private": true,
  "type": "module",
  "scripts": {
    "build": "tsc",
    "start": "node dist/main.js"
  },
  "dependencies": {
    "counter-agent-client": "file:../bridge-sdk/ts/counter-agent-client",
    "effect": "4.0.0-beta.98"
  },
  "devDependencies": {
    "@types/node": "^25",
    "typescript": "^5.9"
  }
}
```

Use an ESM TypeScript configuration such as:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "outDir": "dist",
    "strict": true,
    "skipLibCheck": true
  },
  "include": ["src/**/*.ts"]
}
```

Run `npm install` in the external application after the generated package has been built.

## Invoke an Agent from Effect

Generated external bridge operations return native Promises. `Effect.tryPromise` is the boundary
adapter: its thunk defers the network operation until the Effect runs and turns synchronous throws
or Promise rejections into Effect failures.

```typescript
import { Console, Effect } from "effect";
import { CounterAgent, configure } from "counter-agent-client";

configure({
  server: { type: "local" },
  application: "my-app",
  environment: "local",
});

const program = Effect.gen(function* () {
  // Generated bridge constructors and methods use positional TypeScript arguments.
  const counter = yield* Effect.tryPromise(() => CounterAgent.get("ext-test"));

  // Keep stateful calls sequential by yielding each Promise boundary separately.
  yield* Effect.tryPromise(() => counter.increment());
  yield* Effect.tryPromise(() => counter.increment());
  const last = yield* Effect.tryPromise(() => counter.increment());

  yield* Console.log(last);
  return last;
});

await Effect.runPromise(program);
```

This external program imports Effect from `effect`, but it does not import
`@golemcloud/effect-golem`. The generated package uses `@golemcloud/golem-ts-bridge` transitively
to call Golem's REST API.

### TypeScript Calling Conventions

Effect agent definitions use named records inside the component:

```typescript
Counter.client.get({ name: "ext-test" });
remote.add({ by: 2 });
```

Do not copy that guest-side syntax into the generated external bridge. Generated bridge
constructors and methods are positional, in declaration order:

```typescript
const externalCall = Effect.gen(function* () {
  const counter = yield* Effect.tryPromise(() => CounterAgent.get("ext-test"));
  return yield* Effect.tryPromise(() => counter.add(2));
});
```

Names retain TypeScript casing from the Effect metadata: for example, `CounterAgent`,
`getCurrentValue`, and `repositoryName`. The TypeScript bridge represents WIT `u64` and `s64`
values as JavaScript `bigint`, including values nested inside records, lists, options, and results.
Pass bigint literals such as `1n`; do not coerce 64-bit values to `number`. The bridge uses decimal
integer tokens on the JSON wire and converts responses back to bigint without routing them through
lossy JavaScript numbers.

This is a breaking change for clients generated by older Golem versions, which exposed these WIT
types as `number`. Regenerate and rebuild the bridge, then update callers to pass and consume
`bigint`. Other integer widths remain JavaScript `number`.

## Server Configuration

Call the generated package's `configure` function once before constructing a client:

```typescript
// Local server at http://localhost:9881
configure({
  server: { type: "local" },
  application: "my-app",
  environment: "local",
});

// Golem Cloud
configure({
  server: { type: "cloud", token: process.env.GOLEM_TOKEN! },
  application: "my-app",
  environment: "prod",
});

// Custom deployment
configure({
  server: {
    type: "custom",
    url: "https://my-golem.example.com",
    token: process.env.GOLEM_TOKEN!,
  },
  application: "my-app",
  environment: "prod",
});
```

Use `configure`, not `globalConfig`. Keep tokens out of source control and logs.

## Durable and Phantom Instances

For a durable agent, `get` creates or gets the instance identified by its constructor arguments:

```typescript
const getAgent = Effect.tryPromise(() => MyAgent.get("my-instance"));
```

Every generated agent supports phantom instances:

```typescript
const phantomProgram = Effect.gen(function* () {
  const known = yield* Effect.tryPromise(() =>
    MyAgent.getPhantom(phantomId, "shared-name"),
  );
  const fresh = yield* Effect.tryPromise(() =>
    MyAgent.newPhantom("shared-name"),
  );
  return { known, fresh };
});
```

When the agent declares local configuration, the generated package also provides
`getWithConfig`, `getPhantomWithConfig`, and `newPhantomWithConfig`. Read their generated
declarations because configuration arguments follow the constructor arguments and reflect the
agent's exact config schema.

Generated remote methods are callable Promises for invoke-and-await. They also expose
`abortable(signal, ...args)`, `trigger(...args)`, and `schedule(isoTimestamp, ...args)`. The latter
two return `void`; do not wrap them as if they awaited a result.

## CLI Values for Effect Components

The Golem CLI also uses TypeScript names and values for Effect components:

```shell
golem agent invoke --no-stream 'CounterAgent("ext-test")' increment
golem agent invoke --no-stream 'CounterAgent("ext-test")' add '{"by": 2}'
```

Use exact `defineAgent` and method names, including camelCase. CLI invocation is useful for manual
checks, but the external application should use the generated bridge for typed calls.

## Key Constraints

- Generate `bridge.ts`; there is no Effect-specific external bridge target.
- Use the generated `configure` function and generated class declarations as the source of truth.
- Wrap generated Promise calls in deferred `Effect.tryPromise` thunks.
- Yield stateful calls sequentially when order matters.
- Never use the guest-side `Agent.client`, `HostLive`, or `golem:agent/host` from Node.js.
- Do not expose an HTTP endpoint merely to use the typed external bridge; it calls the agent
  invocation API directly.
- Do not edit generated files under `golem-temp/`; regenerate them with `golem build`.

## Authoritative Effect SDK References

- [Pinned client API](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/Client.ts)
- [Pinned host RPC implementation](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/host/RpcClient.ts)
- [Pinned host runtime layer](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/src/host/HostLive.ts)
- [Pinned Effect v4 package version](https://github.com/golemcloud/effect-golem/blob/4b75f5e4d3cc306c3df75050db93d93aaa379ec3/package.json)
