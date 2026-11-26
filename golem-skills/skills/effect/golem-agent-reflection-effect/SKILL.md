---
name: golem-agent-reflection-effect
description: Composes caller-defined static, discovered, and fully dynamic Golem clients with Effect. Use when schemas are caller-owned or discovered at runtime, or an existing AgentId must be bound to a client.
---

# Runtime reflection with Effect

Normal RPC is the non-reflective baseline and uses the ordinary client from a shared source
definition. Reflection adds caller-defined static, discovered, and fully dynamic clients. Import
`Reflection` from `@golemcloud/effect-golem`. Reflection operations are Effects and their host
requirements flow through the agent dispatcher; compose them in `Effect.gen` rather than running
them as promises.

```ts
import { Effect } from "effect"
import { Reflection } from "@golemcloud/effect-golem"

const callEcho = Effect.gen(function* () {
  const types = yield* Reflection.getAllAgentTypes
  const target = yield* Reflection.getAgentType("ReflectionTarget")
  if (!target) return yield* Effect.fail("ReflectionTarget is not visible")

  const echo = target.method("echo")
  if (!echo) return yield* Effect.fail("echo is not registered")
  if (target.mode !== "durable") return yield* Effect.fail("ReflectionTarget is not durable")
  const client = yield* target.client.upsert({ name: "target" })
  const method = yield* client.method("echo")
  const result = yield* method.invoke({ message: "hello" })
  return { listed: types.some((type) => type.name === target.name), value: result.value }
}).pipe(Effect.scoped)
```

`getAgentType` and `getAgentTypeByAgentId` return `undefined` when the type is not visible in the
current environment; `getAgentTypeByAgentId` reads the type from the type tag of the ID and does
not check that the agent exists. Host and malformed-schema discovery failures are typed Effect
errors. Agent IDs are opaque UUIDs that Golem issues: parse ID text once with `AgentId.parse` and
pass the typed `AgentId` to discovery and binding. Use `constructorInput` and method
`input`/`output` `SchemaRef` values to validate, pack, unpack, or render JSON schemas. Normal
reflected calls use JSON; use the `*Value` variants only for schema-native values. Durable
factories expose `upsert`, `create`, `getById`, `upsertValue`, and `createValue`, and their
clients have an `agentId` Effect. Ephemeral factories expose only `upsert` and `upsertValue`; each
invocation runs on a new one-shot instance whose ID arrives in invocation metadata. Reflected
invocation results include host metadata and a `value` except for unit-returning methods, which
omit it.

`upsert` takes no overrides. Pass creation-time overrides in the options of `create`: `{ keyed, config, env }`, where `config` holds `{ path, value }` entries containing canonical JSON; `createValue` takes schema-native entries. Without `keyed`, `create` makes a new agent without a key, with a new agent ID; with `keyed: true` it creates the keyed agent and fails with a `conflict` cause when it exists. These operations remain Effects and require a scope. Known declarations reject unknown paths, secret fields, and invalid values before opening RPC. An existing agent keeps its initial configuration.

Use `defineAgentClient({ name, id, methods, mode?, config? })` for a caller-defined static full client.
Its `client` has the same lifecycle factories as a `defineAgent` spec (`upsert`, `create`,
`getById`, and `bindSelf` for a durable mode; only `upsert` for an ephemeral mode) and encodes the
constructor input through the caller's schemas before opening RPC. A caller-defined static
method-only `defineAgentClient({ methods })` has no lifecycle factory or discovery; bind it with
`Client.bind(id, definition)`, which reads the agent type from the host by the ID, connects through
`getById`, and assumes durable result semantics. An unimplemented `defineAgent` spec is also a full
client definition, and durable reflected types bind through the same function. `Client.bind`
rejects a full definition of another agent type, and ephemeral specs, ephemeral client
definitions, and reflected ephemeral types, with `ClientBindingError`.

```ts
import { Effect, Schema } from "effect"
import { AgentId, Client, DynamicClient, defineAgentClient, method } from "@golemcloud/effect-golem"
import type * as CoreTypes from "golem:core/types@2.0.0"

const Echo = defineAgentClient({
  name: "Echo", id: { name: Schema.String },
  methods: { echo: method({ input: { message: Schema.String }, success: Schema.String }) },
})

const program = (text: string, inputTree: CoreTypes.SchemaValueTree) => Effect.scoped(Effect.gen(function* () {
  const id = yield* AgentId.parse(text)
  const full = yield* Client.bind(id, Echo)
  const one = yield* full.echo({ message: "full" })
  const methods = defineAgentClient({ methods: Echo.methods })
  const two = yield* (yield* Client.bind(id, methods)).echo({ message: "method only" })
  const dynamic = yield* DynamicClient.bind(id)
  const raw = yield* dynamic.method("echo").invoke(inputTree)
  return { one, two, raw }
}))

// inputTree is supplied by the caller as a native SchemaValueTree.
// raw contains invocation metadata and a native output tree.
```

## Validation and optional values

Effect errors preserve the boundary that failed:

- Client definition compilation and the lifecycle factories encode caller-owned schemas. A full client validates the constructor value and config overrides before opening RPC, and `Client.bind` checks its declared name against the agent type of the ID. A method-only client owns only method schemas.
- Reflected `SchemaRef` packing and invocation apply all discovered restrictions and command constraints locally. Discovery returns immutable metadata and schema snapshots.
- The host remains authoritative for visibility, authorization, resolution of the agent by its key or ID within the environment, effective configuration, and the deployed input schema.
- Awaited calls verify unit/non-unit cardinality and declared output shape. `RemoteCallError`, `ToolRuntimeError`, custom agent errors, and custom tool payloads remain tagged values in the Effect error channel.

In Normal RPC and caller-defined static inputs, use `Schema.optional(...)` in a struct and omit the property. Reflected JSON accepts either an omitted `option<T>` record field or an explicit `null` as absent; re-encoding may include the field with `null`. Reflection JSON Schema does not list that field in `required`:

```ts
import { Effect } from "effect"
import { Reflection } from "@golemcloud/effect-golem"

const optionalCall = Effect.scoped(Effect.gen(function* () {
  const type = yield* Reflection.getAgentType("SearchAgent")
  if (!type || type.mode !== "durable") return yield* Effect.fail("SearchAgent unavailable")
  const client = yield* type.client.upsert({ tenant: "docs" })
  const search = yield* client.method("search")
  const checked = search.definition.input.validateJson({ query: "golem" })
  if (!checked.success) return yield* Effect.fail(checked.issues)
  return yield* search.invoke({ query: "golem" })
}))
```

Canonical JSON represents `s64` and `u64` as base-10 strings. Duration is `{ nanoseconds: "..." }`; quantity uses a decimal-string `mantissa`. Smaller integers remain JSON numbers. JSON Schema projections expose the same canonical patterns and exact range metadata.

Capabilities, futures, and streams cannot be packed or unpacked as reflected JSON. Their reflection JSON Schema projection is unsatisfiable; use schema-native value APIs for those leaves.

## Cancellation, streams, and cleanup

Agent calls are Effects: interrupting an awaited call withdraws the invocation on a best-effort basis while it has not started, and only stops waiting after it started, so remote side effects may already have happened. `trigger` and `schedule` (and `triggerValue` / `scheduleValue`) succeed with an `Invocation` handle once the target accepted the invocation. The current scope owns the handle; closing the scope only detaches. Use its `status`, `awaitResult` and `cancel` Effects while the scope is open:

```ts
import { Effect } from "effect"
import { Reflection } from "@golemcloud/effect-golem"

const scheduled = Effect.scoped(Effect.gen(function* () {
  const type = yield* Reflection.getAgentType("CounterAgent")
  if (!type || type.mode !== "durable") return yield* Effect.fail("CounterAgent unavailable")
  const client = yield* type.client.upsert({ name: "main" })
  const add = yield* client.method("add")
  const scheduled = yield* add.schedule({ seconds: 1n, nanoseconds: 0 }, { by: 2 })
  const withdrawn = yield* scheduled.cancel // false once it finished; fails while it runs
}))
```

Streams and opaque capabilities have no canonical JSON form. Use the `*Value` APIs, transfer an owned handle once, and consume returned streams inside the scope. Reflected tool `startJson`/`startValue` exposes independent `stdout`, `stderr`, `result`, `collect`, and `cancel` Effects. `collect` concurrently waits for the result and both outputs and gives the result error precedence. Scope closure releases observers and owned handles; invoke `cancel` when the remote operation itself must be cancelled.

## Discovery to a fully dynamic agent

Retain the discovered method snapshot and apply it explicitly around the
dynamic call. `DynamicClient.bind(id)` does not inherit the snapshot's validation
policy:

```ts
import { Effect } from "effect"
import { DynamicClient, Reflection } from "@golemcloud/effect-golem"

const dynamicSearch = Effect.scoped(Effect.gen(function* () {
  const type = yield* Reflection.getAgentType("SearchAgent")
  const method = type?.method("search")
  if (!type || type.mode !== "durable" || !method)
    return yield* Effect.fail("SearchAgent.search is unavailable")

  const input = method.input.packJson({ query: "golem", cursor: null })

  const keyed = yield* type.client.upsert({ tenant: "docs" })
  const dynamic = yield* DynamicClient.bind(yield* keyed.agentId)
  const result = yield* dynamic.method(method.name).invoke(input).pipe(
    Effect.catch((error) =>
      Effect.logError("dynamic search failed", error).pipe(
        Effect.andThen(Effect.fail(error)),
      ),
    ),
  )
  if (!method.output || result.value === undefined)
    return yield* Effect.fail("search returned an unexpected unit result")
  const outputCheck = method.output.validateValue(result.value)
  if (!outputCheck.success) return yield* Effect.fail(outputCheck.issues)
  return method.output.unpackJson(result.value)
}))
```

`Effect.scoped` releases the dynamic RPC connection. If packed values carry
owned streams, transfer each input once and consume or close every returned
stream before the scope ends.
