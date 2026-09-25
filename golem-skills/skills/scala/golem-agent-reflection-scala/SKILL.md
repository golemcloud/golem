---
name: golem-agent-reflection-scala
description: "Discovering and calling Golem agents through runtime reflection in Scala. Use when agent types or methods are selected dynamically, schemas must be inspected at runtime, caller-owned codecs are needed, or SchemaValue calls must avoid discovery."
---

# Agent Reflection and Client Approaches (Scala)

Golem exposes four composable client approaches:

- **Normal RPC** uses a shared source definition through the Scala SDK's ordinary typed RPC mechanism. It is not reflection.
- **Caller-defined static clients** use caller-owned compile-time codecs through `AgentClientDefinition`, without deployment discovery. Definitions may be method-only or full.
- **Discovered clients** read the current environment and produce immutable, snapshot-backed `ReflectedAgentClient` and `ReflectedToolClient` values with automatic input and output validation.
- **Fully dynamic clients** use `DynamicAgentClient` or `DynamicToolClient` to transport caller-packed schema-native values without retaining a schema authority.

The last three are reflection approaches. They are composable: discovery can
select schemas and identities for a later fully dynamic call, and a durable
`ParsedAgentId` can be rebound through a method-only, discovered, or fully
dynamic client.

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
An omitted option record field and an explicit `null` both decode as `None`;
re-encoding may emit `null`, and the field is omitted from JSON Schema
`required`. Canonical JSON encodes `s64` and `u64` as decimal strings, duration
as `{ "nanoseconds": "..." }`, and quantity mantissas as decimal strings. These
strings reject `+`, leading zeroes, `-0`, and overflow. Capabilities, futures,
and streams cannot be packed or unpacked as reflected JSON and project to an
unsatisfiable reflection JSON Schema.
`getAgentType` returns `Right(None)` for a missing type and reserves `Left` for
discovery or decoding failures. The returned schemas are an immutable snapshot;
call discovery again when a newer deployment must be observed.

## Use discovered clients

JSON convenience automatically packs and unpacks:

```scala
val invocation = client.method("add").flatMap { add =>
  // The returned Future contains either a reflection error or JSON invocation.
  Right(add.invokeJson(Json.Object("by" -> Json.Number(BigDecimal(5)))))
}
```

For explicit reflected packing, call `method.definition.input.packJson`, then
`invokeValue`; after awaiting, call the output `SchemaRef.unpackJson`.

For fully dynamic values, bind `agentId.dynamicClient` and manually construct the
positional record:

```scala
import golem.schema.SchemaValue

val call = agentId.dynamicClient.map(
  _.method("add").invokeValue(
    SchemaValue.RecordValue(List(SchemaValue.U32Value(5)))
  )
)
```

Fully dynamic clients never discover or validate schemas. Constructor and method
record fields must be packed in declaration order; the runtime authoritatively
accepts or rejects the attempt.

## Discover a tool, then invoke it dynamically

This bridge deliberately carries selected discovery values into a fully
dynamic client. It packs and validates before the call, handles transport and
tool errors, then explicitly validates and decodes the raw result:

```scala
import golem.reflection.{DynamicToolClient, Reflection}
import golem.schema.TypedSchemaValue
import zio.blocks.schema.json.Json

import scala.concurrent.{ExecutionContext, Future}

def invokeDiscoveredToolDynamically(
  toolName: String,
  path: List[String],
  json: Json
)(using ExecutionContext): Future[Either[String, Option[Json]]] = {
  val prepared = for {
    tool    <- Reflection.getToolType(toolName).left.map(_.toString)
    command <- tool.command(path).left.map(_.toString)
    input   <- command.packJson(json).left.map(_.toString)
    _       <- command.inputSchema
                 .validateValue(input)
                 .left
                 .map(_.map(_.message).mkString("; "))
  } yield (tool, command, TypedSchemaValue(command.inputSchema.graph, input))

  prepared match {
    case Left(error) => Future.successful(Left(error))
    case Right((tool, command, input)) =>
      new DynamicToolClient(tool.lookupName).invoke(command.path, input).map {
        case Left(error) => Left(error.toString)
        case Right(raw)  =>
          (command.result, raw.result) match {
            case (None, None) => Right(None)
            case (Some(schema), Some(output)) =>
              schema
                .validateValue(output.value)
                .left
                .map(_.map(_.message).mkString("; "))
                .flatMap(_ => schema.unpackJson(output.value).left.map(_.message))
                .map(Some(_))
            case _ => Left("missing or unexpected tool result")
          }
      }
  }
}
```

Moving `tool.lookupName`, `command.path`, and schema values into
`DynamicToolClient` does not transfer the reflected client's validation policy.
This non-streaming recipe owns no stream handles. For dynamic streaming calls,
drain or close transferred streams and cancel a started invocation when it is
abandoned.

Reflected factories accept optional creation-time overrides: `get(json, config)` and `getValue(value, overrides)` use canonical JSON entries or schema-native `ConfigOverride` entries, respectively. The JSON entries are `ReflectedConfigJson(path, value)`. The reflected type checks declared paths, secret fields, and values before constructing RPC. `AgentType.bind(id, overrides)` accepts validated native entries for an existing identity.

All reflected and direct methods support awaited, trigger, and scheduled calls
through `invokeValue`, `triggerValue`, and `scheduleValue`. Reflected live
streams are supported by awaited value calls. Trigger and scheduled discovered
or caller-defined calls reject methods whose input or output schema contains a
stream.

## Define a caller-defined static client

`AgentClientDefinition` has two capability-typed construction forms and does
not discover remote schemas. `InputRecordCodec` and `OutputCodec` are the
caller's schema authority.

A method-only client definition contains methods only. It binds an existing durable
`ParsedAgentId`, taking the name and constructor value from that identity:

```scala
import golem.reflection._
import golem.runtime.{InputRecordCodec, OutputCodec}

val methods: AgentClientDefinition[MethodOnly, Unit, NoConfig] =
  AgentClientDefinition.methodOnly
val add = methods.method(
  "add",
  InputRecordCodec.single[Int]("by"),
  OutputCodec.single[Int]
)

val counter = methods.bind(existingId)
val result = counter.map(_.method(add).invoke(5))
```

A full client adds the declared name, lifecycle mode, constructor codec,
and an optional typed config codec. Construction infers `DurableFull` or
`EphemeralFull`; both extend `Full`, which gates identity helpers and
lifecycle factories:

```scala
val counter = AgentClientDefinition.full(
  name = "CounterAgent",
  mode = AgentMode.Durable,
  constructor = InputRecordCodec.single[String]("name")
)

val ordinary = counter.client.get("main")
val known = counter.client.getPhantom("main", phantomId)
val fresh = counter.client.newPhantom("main")
val id = counter.agentId("main", Some(phantomId))
val bound = counter.bind(existingId)
```

Fully defined factories also accept a typed config value as a second argument when the definition declares an `AgentConfigCodec`. Fully defined clients use `bindWithConfig(existingId, config)` for typed overrides; method-only clients use `bindWithOverrides(existingId, entries)` with raw `ConfigOverride` entries because they have no local declarations to validate against.

Overrides are optional: component defaults can satisfy required local declarations. The host checks the effective config when it creates the worker and supplies secrets. An existing durable worker retains its persisted initial config, so passing overrides while binding its ID does not reconfigure it.

Full durable binding checks both the declared name and the constructor value
against the declared constructor codec before creating transport. Fully defined
ephemeral definitions support logical-new and known-phantom factories and
have no `CanBindAgentClient` capability, so generic binding is rejected at
compile time. `AgentConfigCodec[C]` maps a typed config carrier to
the existing typed `ConfigOverride` values; secret fields should not be exposed
by that codec.

## Identity and lifecycle boundaries

- Use a supplied `ParsedAgentId` directly or inspect it with `parts`.
- Use `ParsedAgentId.create` for fully dynamic durable, known-phantom, or newly
  generated phantom identities.
- Discovered, normal RPC, and full caller-defined factories provide `get`
  (durable only), known `getPhantom`, and `newPhantom`.
- Use `DynamicAgentClient.ephemeral(typeName, constructorValue)`
  for a raw ephemeral invocation address.

Method-only and fully dynamic agent clients do not expose lifecycle factories.
Packed streams and owned handles still follow the Scala SDK's transfer and
cleanup rules.

An ephemeral address has no guaranteed reusable pre-invocation identity. The
final identity comes from invocation metadata and must not be treated as a
resumable durable identity or rebound. Durable identities from invocation
metadata may be discovered later and rebound through a reflected client.
