---
name: golem-agent-reflection-effect
description: Discovers and invokes Golem agents through Effect-native runtime reflection. Use when agent types or methods are selected dynamically or their schemas must be inspected at runtime.
---

# Runtime reflection with Effect

Import `Reflection` from `@golemcloud/effect-golem`. Reflection operations are Effects and their
host requirements flow through the agent dispatcher; compose them in `Effect.gen` rather than
running them as promises.

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

An unimplemented `defineAgent` spec is a complete caller-owned contract. Its `agentId(input)`
creates a parsed identity, and `Client.bind(identity, spec)` checks the exact name and constructor
schema locally before opening RPC. A method-only `Client.contract({ methods })` has no lifecycle
factory or discovery: `Client.bind(identity, contract)` assumes durable result semantics. Durable
reflected types bind through the same function after schema validation. Complete ephemeral specs
and reflected ephemeral types reject generic existing-ID binding; use known/fresh phantom
factories. Constructing an ephemeral identity requires a phantom ID.

```ts
import { Effect, Schema } from "effect"
import { AgentIdentity, Client, DynamicClient, defineAgent, method } from "@golemcloud/effect-golem"
import type * as CoreTypes from "golem:core/types@2.0.0"

const Echo = defineAgent({
  name: "Echo", id: { name: Schema.String },
  methods: { echo: method({ input: { message: Schema.String }, success: Schema.String }) },
}) // specification only; no .implement call

const program = (inputTree: CoreTypes.SchemaValueTree) => Effect.scoped(Effect.gen(function* () {
  const identity = yield* Echo.agentId({ name: "main" })
  const exact = yield* Client.bind(identity, Echo)
  const one = yield* exact.echo({ message: "exact" })
  const methods = Client.contract({ methods: Echo.methods })
  const two = yield* (yield* Client.bind(identity, methods)).echo({ message: "method only" })
  const parsed = yield* AgentIdentity.parse(identity.encoded)
  const dynamic = yield* DynamicClient.bind(parsed)
  const raw = yield* dynamic.method("echo").invoke(inputTree)
  return { one, two, raw }
}))

// inputTree is supplied by the caller as a native SchemaValueTree.
// raw contains invocation metadata and a native output tree.
```
