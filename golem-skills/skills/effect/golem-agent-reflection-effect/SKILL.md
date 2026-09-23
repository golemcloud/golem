---
name: golem-agent-reflection-effect
description: Composes caller-defined static, discovered, and fully dynamic Golem clients with Effect. Use when schemas are caller-owned or discovered at runtime, or a durable identity must be rebound.
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
  const client = yield* target.client.get({ name: "target" })
  const method = yield* client.method("echo")
  const result = yield* method.invoke({ message: "hello" })
  return { listed: types.some((type) => type.name === target.name), value: result.value }
}).pipe(Effect.scoped)
```

`getAgentType` and `getAgentTypeByAgentId` return `undefined` when the type is not visible in the
current environment. Host and malformed-schema discovery failures are typed Effect errors. Agent
identity strings are environment-scoped: parse them with `AgentIdentity.parse` before
identity-specific discovery or binding. Use `constructorInput` and method `input`/`output`
`SchemaRef` values to validate, pack, unpack, or render JSON schemas. Normal reflected calls use
JSON; use the `*Value` variants only for schema-native values. Durable factories expose `get` and
phantom operations; ephemeral factories expose `getPhantom` for known IDs and `newPhantom` for
fresh logical clients, but no ordinary `get`. Reflected invocation results include host metadata
and a `value` except for unit-returning methods, which omit it.

Pass optional creation-time overrides as a second argument to reflected factories. Use `{ path, value }` entries containing canonical JSON with `get`, `getPhantom`, or `newPhantom`; use the `*Value` factory variants for schema-native values. For an existing identity, use `target.bindWithJsonConfig(identity, entries)` or `target.bindWithConfig(identity, nativeEntries)`. A fully defined client exposes `bindWithConfig(identity, { overrides })`; a method-only client exposes `bindWithEntries(identity, nativeEntries)`. These operations remain Effects and require a scope. Known declarations reject unknown paths, secret fields, and invalid values before opening RPC. An existing durable worker retains its initial configuration even if overrides are passed while binding.

Use `defineAgentClient({ name, id, methods, mode?, config? })` for a caller-defined static full client.
Its `agentId(input)` creates a parsed identity, and `identity.client(clientDefinition)` checks the declared
name and constructor schema locally before opening RPC. A caller-defined static method-only
`defineAgentClient({ methods })` has no lifecycle factory or discovery; binding assumes durable
result semantics. An unimplemented `defineAgent` spec is also a full client definition. Durable
reflected types bind through the same function after schema validation. Fully defined ephemeral specs
and reflected ephemeral types reject generic existing-ID binding; use known/fresh phantom
factories. Constructing an ephemeral identity requires a phantom ID.

```ts
import { Effect, Schema } from "effect"
import { AgentIdentity, defineAgentClient, method } from "@golemcloud/effect-golem"
import type * as CoreTypes from "golem:core/types@2.0.0"

const Echo = defineAgentClient({
  name: "Echo", id: { name: Schema.String },
  methods: { echo: method({ input: { message: Schema.String }, success: Schema.String }) },
})

const program = (inputTree: CoreTypes.SchemaValueTree) => Effect.scoped(Effect.gen(function* () {
  const identity = yield* Echo.agentId({ name: "main" })
  const full = yield* identity.client(Echo)
  const one = yield* full.echo({ message: "full" })
  const methods = defineAgentClient({ methods: Echo.methods })
  const two = yield* (yield* identity.client(methods)).echo({ message: "method only" })
  const parsed = yield* AgentIdentity.parse(identity.encoded)
  const dynamic = yield* parsed.dynamicClient()
  const raw = yield* dynamic.method("echo").invoke(inputTree)
  return { one, two, raw }
}))

// inputTree is supplied by the caller as a native SchemaValueTree.
// raw contains invocation metadata and a native output tree.
```

## Validation and optional values

Effect errors preserve the boundary that failed:

- Client definition compilation and `agentId` encode caller-owned schemas. A full client validates the declared name, constructor value, and config overrides before opening RPC. A method-only client owns only method schemas and raw typed config entries.
- Reflected `SchemaRef` packing and invocation apply all discovered restrictions and command constraints locally. Discovery returns immutable metadata and schema snapshots.
- The host remains authoritative for visibility, authorization, environment-scoped identity resolution, effective configuration, and the deployed input schema.
- Awaited calls verify unit/non-unit cardinality and declared output shape. `RemoteCallError`, `ToolRuntimeError`, custom agent errors, and custom tool payloads remain tagged values in the Effect error channel.

In Normal RPC and caller-defined static inputs, use `Schema.optional(...)` in a struct and omit the property. Reflected JSON accepts either an omitted `option<T>` record field or an explicit `null` as absent; re-encoding may include the field with `null`. Reflection JSON Schema does not list that field in `required`:

```ts
import { Effect } from "effect"
import { Reflection } from "@golemcloud/effect-golem"

const optionalCall = Effect.scoped(Effect.gen(function* () {
  const type = yield* Reflection.getAgentType("SearchAgent")
  if (!type || type.mode !== "durable") return yield* Effect.fail("SearchAgent unavailable")
  const client = yield* type.client.get({ tenant: "docs" })
  const search = yield* client.method("search")
  const checked = search.definition.input.validateJson({ query: "golem" })
  if (!checked.success) return yield* Effect.fail(checked.issues)
  return yield* search.invoke({ query: "golem" })
}))
```

Canonical JSON represents `s64` and `u64` as base-10 strings. Duration is `{ nanoseconds: "..." }`; quantity uses a decimal-string `mantissa`. Smaller integers remain JSON numbers. JSON Schema projections expose the same canonical patterns and exact range metadata.

Capabilities, futures, and streams cannot be packed or unpacked as reflected JSON. Their reflection JSON Schema projection is unsatisfiable; use schema-native value APIs for those leaves.

## Cancellation, streams, and cleanup

Agent calls are Effects: fiber interruption cancels result observation, while remote side effects may already have happened. A scheduled call returns a scoped cancel Effect. Keep the scope open until the cancellation token is no longer needed:

```ts
import { Effect } from "effect"
import { Reflection } from "@golemcloud/effect-golem"

const scheduled = Effect.scoped(Effect.gen(function* () {
  const type = yield* Reflection.getAgentType("CounterAgent")
  if (!type || type.mode !== "durable") return yield* Effect.fail("CounterAgent unavailable")
  const client = yield* type.client.get({ name: "main" })
  const add = yield* client.method("add")
  const pending = yield* add.schedule({ seconds: 1n, nanoseconds: 0 }, { by: 2 })
  yield* pending.cancel
}))
```

Streams and opaque capabilities have no canonical JSON form. Use the `*Value` APIs, transfer an owned handle once, and consume returned streams inside the scope. Reflected tool `startJson`/`startValue` exposes independent `stdout`, `result`, `collect`, and `cancel` Effects. `collect` waits for both result and stdout and gives the result error precedence. Scope closure releases observers and owned handles; invoke `cancel` when the remote operation itself must be cancelled.

## Discovery to a fully dynamic agent

Retain the discovered method snapshot and apply it explicitly around the
dynamic call. `dynamicClient()` does not inherit the snapshot's validation
policy:

```ts
import { Effect } from "effect"
import { Reflection } from "@golemcloud/effect-golem"

const dynamicSearch = Effect.scoped(Effect.gen(function* () {
  const type = yield* Reflection.getAgentType("SearchAgent")
  const method = type?.method("search")
  if (!type || type.mode !== "durable" || !method)
    return yield* Effect.fail("SearchAgent.search is unavailable")

  const input = method.input.packJson({ query: "golem", cursor: null })
  const inputCheck = method.input.validateValue(input)
  if (!inputCheck.success) return yield* Effect.fail(inputCheck.issues)

  const identity = yield* type.agentId({ tenant: "docs" })
  const dynamic = yield* identity.dynamicClient()
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
