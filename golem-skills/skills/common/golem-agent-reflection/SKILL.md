---
name: golem-agent-reflection
description: "Choosing Golem agent reflection levels and identity lookup behavior across SDKs. Use when agent types or methods are discovered dynamically, schemas are inspected at runtime, or an environment-scoped agent identity must be resolved."
---

# Agent Reflection

Use the narrowest client level that matches what the caller knows:

| Level | Choose it when | Identity and lifecycle | Validation |
|---|---|---|---|
| 1: generated or definition-owned client | The target definition is available in source | The definition owns durable, phantom, and ephemeral factories allowed by its mode | Generated codecs validate typed inputs and decode declared outputs |
| 2a: method-only client | Methods are known locally and an existing durable canonical or phantom identity is already available | It only binds that identity; it has no name, constructor, mode, config declarations, or factories | Local method codecs validate calls and outputs; identity shape and configuration declarations are intentionally unknown |
| 2b: full client | Name, constructor, methods, mode, and optional config declarations are known, but no implementation is imported | It constructs identities and owns the same lifecycle factories as a Level 1 definition | It additionally checks exact type name, constructor shape, and declared configuration locally |
| 3: reflected client | The deployed type or method is selected at runtime | Discovery returns factories for the discovered mode; lookup by identity never creates the target | Immutable discovered schemas validate canonical JSON or schema-native values before calls and validate declared output cardinality and shape afterward |
| 4: dynamic client | Infrastructure deliberately invokes arbitrary methods with schema-native values | It starts only from an existing durable canonical or phantom identity and neither discovers nor creates targets | The caller owns schema correctness; the host still performs authorization and target-side validation |

Level 2 means a caller-owned client, with two deliberately different options: method-only and full.
Do not describe either option as a “contract” in public APIs or diagnostics.

Agent identity strings are environment-scoped. Reflection identities do not include a component ID: the runtime resolves the agent type's implementing component within the caller's environment. Component-bearing IDs belong to lower-level host-management APIs, not reflection clients.

Constructor identity values contain only caller-supplied fields. The host injects principal fields separately, so a principal-scoped agent can be reconstructed from an identity returned by invocation metadata. A known ephemeral phantom can be constructed once; use the invocation metadata for its final identity. A final ephemeral identity cannot be bound for another call.

Discovery lookups are optional: a name or identity lookup returns no type when the deployment is missing, the identity is malformed, or the caller cannot view it. Parsing an identity is strict and reports malformed input. Identity discovery never creates the target agent.

Reflected schema graphs and invocation metadata are immutable snapshots. Validate or pack JSON through the reflected constructor or method schema, and treat a missing, extra, graph-incompatible, or malformed declared output as a remote output error.

Creation-time configuration overrides are optional because component defaults may satisfy required local declarations. Reflected and full clients validate known declaration paths, values, and secret restrictions locally before opening RPC. Method-only clients may carry raw typed entries without claiming declaration-aware validation. The host validates the effective configuration when creating the worker, provisions secrets, authorizes the call, and validates the target-side input. An existing durable worker keeps its persisted initial configuration; supplying overrides when binding its ID does not reconfigure it.

## Validation boundaries

- Definition and metadata construction reject malformed schema graphs, duplicate command surfaces, invalid defaults, unresolved references, incompatible `ValueIs` literals, and impossible command restrictions.
- Typed and full clients encode through their local schemas before opening RPC. Full-client binding also checks the exact type name and constructor value. Method-only binding cannot make either claim.
- Reflected clients validate all discovered numeric, text, binary, path, URL, quantity, collection, discriminator, optional, default, and command-constraint rules before opening RPC. Namespace tool nodes stay discoverable but have no callable body.
- The host remains authoritative for visibility, authorization, effective configuration, durable identity resolution, and the deployed target schema.
- Awaited calls validate output cardinality and shape. Structured remote agent/tool errors and custom payloads stay structured rather than being reduced to messages.

## Optional values and canonical JSON

Typed Level 1 and Level 2 clients use the language's normal optional-field syntax. Reflected JSON is schema-shaped instead: records contain every declared field and an absent `option<T>` is `null`. Tool defaults do not turn `Present` into a key-existence check; constraints evaluate the effective optional/default carrier, and nested `ValueIs` comparisons use the declared nested schema.

Canonical invocation JSON uses JSON numbers for integers through 32 bits. Signed and unsigned 64-bit integers are canonical base-10 strings, as are duration nanoseconds and quantity mantissas. A duration is `{ "nanoseconds": "..." }`; a quantity is `{ "mantissa": "...", "scale": number, "unit": string }`. Leading zeroes, `+`, and `-0` are invalid. JSON Schema projections use the same string patterns and carry the exact range as metadata.

Streams and opaque capabilities have no canonical JSON representation. Use the schema-native `*Value` APIs, transfer each owned handle once, consume returned streams, and cancel or close started operations according to the language-specific API.

Load the language-specific reflection skill for concrete SDK APIs and examples.
