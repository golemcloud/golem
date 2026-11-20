---
name: golem-agent-reflection
description: "Choosing and composing Golem client approaches across SDKs. Use when caller-defined schemas, runtime discovery, fully dynamic invocation, or connecting to an existing agent by its agent ID is needed."
---

# Agent Reflection

There are four client approaches. Normal RPC is the non-reflective baseline; the other three are
reflection approaches that can be composed:

| Client approach | Choose it when | Identity and lifecycle | Validation |
|---|---|---|---|
| Normal RPC | Producer and caller share a source definition through the SDK's ordinary RPC mechanism | The shared definition owns the `upsert`, `create`, and `get-by-id` factories allowed by its mode (ephemeral types have only `upsert`) | Its static codecs validate typed inputs and decode declared outputs |
| Caller-defined static clients | The caller owns compile-time schemas and codecs without deployment discovery | Method-only agent clients connect to an existing durable agent by its agent ID; full agent clients own lifecycle factories; typed tool definitions select commands | Local codecs validate calls and outputs; full clients also check the declared type name, constructor shape, and configuration |
| Discovered clients | The deployed agent type, method, tool, or command is selected from the current environment | Discovery returns immutable, snapshot-backed `Reflected*` clients and factories for the discovered mode | The retained snapshot automatically validates inputs and declared outputs |
| Fully dynamic clients | Infrastructure transports already packed schema-native values without retaining a schema authority | `DynamicAgentClient` connects to an existing durable agent by its agent ID; `DynamicToolClient` invokes an explicit tool name and command path | The caller owns packing and validation policy; the host still performs authorization and target-side validation |

Caller-defined static agent clients have two deliberately different options: method-only and full.
Caller-defined static tool clients use caller-owned typed definitions.

An agent ID is a UUID that Golem issues; it carries a tag of the agent's type but not its constructor parameters, so no SDK computes it offline. Agents are stored and addressed per environment and agent ID. Get an ID from a client (`agentId` / `agent_id`), from invocation metadata, or from agent metadata. A keyed agent is also reachable by its key, the canonical `Type(parameters)` text of its constructor parameters; `upsert` always uses a key. An agent created without a key is reachable only by its ID.

Constructor values contain only caller-supplied fields. The host injects principal fields separately. An ephemeral client is a logical proxy: each invocation runs on a new one-shot instance whose agent ID only the invocation metadata reports, and that ID cannot be used for another call.

Discovery lookups are optional: a name or agent-ID lookup returns no type when the deployment is missing or the caller cannot view it. Parsing an agent ID is strict and reports malformed input. Neither discovery nor connecting by ID ever creates the target agent; an unknown ID, or the ID of an agent of another type, is a typed not-found error. A keyed create of an existing key is a typed conflict error.

Reflected schema graphs and invocation metadata are immutable snapshots. Validate or pack JSON through the reflected constructor or method schema, and treat a missing, extra, graph-incompatible, or malformed declared output as a remote output error. Rediscover explicitly when a fresh deployment snapshot is required; a client never refreshes itself automatically.

Creation-time configuration overrides are optional because component defaults may satisfy required local declarations. Reflected and full clients validate known declaration paths, values, and secret restrictions locally before opening RPC. Method-only clients may carry raw typed entries without claiming declaration-aware validation. The host validates the effective configuration when creating the worker, provisions secrets, authorizes the call, and validates the target-side input. Overrides exist only on `create`: `upsert` and connecting by ID take none, and an existing durable agent keeps its persisted initial configuration.

## Validation boundaries

- Definition and metadata construction reject malformed schema graphs, duplicate command surfaces, invalid defaults, unresolved references, incompatible `ValueIs` literals, and impossible command restrictions.
- Normal RPC and caller-defined static clients encode through their local schemas before opening RPC. A full client also checks its constructor value, and connects by ID only to an agent of its declared type. A method-only client connects to an agent of any type.
- Reflected clients validate all discovered numeric, text, binary, path, URL, quantity, collection, discriminator, optional, default, and command-constraint rules before opening RPC. Namespace tool nodes stay discoverable but have no callable body.
- The host remains authoritative for visibility, authorization, effective configuration, durable identity resolution, and the deployed target schema.
- Awaited calls validate output cardinality and shape. Structured remote agent/tool errors and custom payloads stay structured rather than being reduced to messages.

## Optional values and canonical JSON

Normal RPC and caller-defined static clients use the language's normal optional-field syntax. In reflected JSON, an omitted `option<T>` record field and an explicit `null` both decode as absent; re-encoding may include the field with `null`. Reflection JSON Schema therefore omits optional fields from `required`. In the canonical JSON input of a reflected tool command, an omitted scalar option, positional with a declared default, or flag takes its declared default (a count flag takes 0); the command input JSON Schema omits these arguments from `required` and states their `default`. The tail and repeatable options are not defaulted this way. Tool defaults do not turn `Present` into a key-existence check; constraints evaluate the effective optional/default carrier, and nested `ValueIs` comparisons use the declared nested schema.

Canonical invocation JSON uses JSON numbers for integers through 32 bits. Signed and unsigned 64-bit integers are canonical base-10 strings, as are duration nanoseconds and quantity mantissas. A duration is `{ "nanoseconds": "..." }`; a quantity is `{ "mantissa": "...", "scale": number, "unit": string }`. Leading zeroes, `+`, and `-0` are invalid. JSON Schema projections use the same string patterns and carry the exact range as metadata.

Streams, futures, and opaque capabilities have no canonical JSON representation. Their reflection JSON Schema projection is unsatisfiable. Use the schema-native `*Value` APIs, transfer each owned handle once, consume returned streams, and cancel or close started operations according to the language-specific API.

## Mixing client approaches

The approaches are surfaces that compose around schema snapshots and durable agent IDs:

```text
shared source definition ──Normal RPC──────────────┐
full caller-defined client ──factory───────────────┼─> AgentId
environment discovery ──snapshot─> Reflected* ─────┘       │
          │                                                 ├─> method-only get-by-id
          ├─> select schema → pack → Dynamic* → validate ───┼─> reflected get-by-id
          └─> rediscover later from invocation metadata ────└─> dynamic get-by-id
```

- Discovery to a reflected client retains automatic snapshot-based input and output validation.
- Discovery to a fully dynamic client is explicit: select the input/output schemas, pack and validate the input, invoke through `Dynamic*`, then validate and decode the result with the saved snapshot.
- A durable client of Normal RPC or of a full caller-defined or discovered factory returns its `AgentId`. That ID can later connect a method-only, reflected, or fully dynamic client.
- The agent ID of invocation metadata can be rediscovered later (the agent type comes from its type tag), then connected as a reflected client when the agent is durable.
- Tool discovery can feed `DynamicToolClient`: preserve the discovered tool name, canonical command path, input graph, and result graph; pack and validate around the dynamic call yourself.

Moving values or schemas obtained from discovery into a fully dynamic client does not transfer the
reflected client's automatic validation policy. Method-only and fully dynamic agent clients expose
no lifecycle factories. The agent ID of an ephemeral one-shot instance cannot be used for another call. Packed streams and owned
handles still follow the SDK's transfer, consumption, cancellation, and cleanup rules.

Load the language-specific reflection skill for concrete SDK APIs and examples.
