---
name: golem-agent-reflection-scala
description: "Discovering and calling Golem agents through runtime reflection in Scala. Use when agent types or methods are selected dynamically, schemas must be inspected at runtime, caller-owned codecs are needed, or SchemaValue calls must avoid discovery."
---

# Calling Agents with Runtime Reflection (Scala)

Use generated clients when the complete target definition is available at
compile time. Otherwise choose one authority and invocation path explicitly:
caller-authored codecs, runtime-reflected schemas, or direct schema-free
`SchemaValue` calls.

## Discover and inspect schemas

```scala
import golem.reflection.Reflection

val target = Reflection.getAgentType("CounterAgent").flatMap(
  _.toRight(golem.reflection.GolemReflectError.Discovery("CounterAgent is unavailable"))
)
val method = target.flatMap(
  _.method("add").toRight(golem.reflection.GolemReflectError.Discovery("add is unavailable"))
)
```

Agent type names are unique in an environment. An `AgentType` exposes its
current component ID, lifecycle mode, constructor
`SchemaRef`, and method `SchemaRef`s. `SchemaRef` validates `SchemaValue`, packs
and unpacks canonical `zio.blocks.schema.json.Json`, and renders JSON Schema.
`getAgentType` returns `Right(None)` for a missing type and reserves `Left` for
discovery or decoding failures.

## Use the three reflected/value invocation paths

JSON convenience automatically packs and unpacks:

```scala
val invocation = client.method("add").flatMap { add =>
  // The returned Future contains either a reflection error or JSON invocation.
  Right(add.invokeJson(Json.Object("by" -> Json.Number(BigDecimal(5)))))
}
```

For explicit reflected packing, call `method.definition.input.packJson`, then
`invokeValue`; after awaiting, call the output `SchemaRef.unpackJson`.

For direct values, bind `agentId.dynamicClient` and manually construct the
positional record:

```scala
import golem.schema.SchemaValue

val call = agentId.dynamicClient.map(
  _.method("add").invokeValue(
    SchemaValue.RecordValue(List(SchemaValue.U32Value(5)))
  )
)
```

Direct clients never discover or validate schemas. Constructor and method
record fields must be packed in declaration order; the runtime authoritatively
accepts or rejects the attempt.

All reflected and direct methods support awaited, trigger, and scheduled calls
through `invokeValue`, `triggerValue`, and `scheduleValue`. Reflected live
streams are supported by awaited value calls. Trigger and scheduled reflected
or caller-codec calls reject methods whose input or output schema contains a
stream.

## Define a caller-codec typed contract

`AgentClientDefinition` does not discover remote schemas. `InputRecordCodec`
and `OutputCodec` are the caller's schema authority; the environment-unique
type name is used only to resolve current implementation identity metadata:

```scala
import golem.reflection._
import golem.runtime.{InputRecordCodec, OutputCodec}

val contract = AgentClientDefinition(
  "CounterAgent",
  InputRecordCodec.single[String]("name")
)
val add = contract.method(
  "add",
  InputRecordCodec.single[Int]("by"),
  OutputCodec.single[Int]
)

val counter = contract.client.get("main")
val result = counter.map(_.method(add).invoke(5))
```

Use `agentId.client(contract)` to bind the same caller-owned codecs to an
existing durable identity. Both creation and binding fail when the named type
is not registered in the current environment.

## Lifecycle attempts

- Use a supplied `AgentId` directly or inspect it with `parts`.
- Use `AgentId.create` for schema-free durable, known-phantom, or newly
  generated phantom identities.
- Reflected and caller-codec factories provide `get`, `getPhantom`, and
  `newPhantom`.
- Use `DynamicAgentClient.ephemeral(componentId, typeName, constructorValue)`
  for a raw ephemeral invocation address.

An ephemeral address has no guaranteed reusable pre-invocation identity. The
final identity comes from invocation metadata and must not be treated as a
resumable durable identity.
