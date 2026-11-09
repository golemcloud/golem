---
name: golem-agent-reflection-kotlin
description: "Discovering and calling Golem agents through runtime reflection in Kotlin. Use when agent types or methods are selected dynamically, schemas must be inspected at runtime, or an agent must be called by type name without a generated or typed client."
---

# Agent Reflection in Kotlin

`cloud.golem.runtime.AgentReflection` reads the agent types registered with the Golem host and builds a dynamic client for any of them, without a `@RemoteAgent` interface or a generated bridge client.

## Discover agent types

```kotlin
import cloud.golem.runtime.AgentReflection

val all = AgentReflection.agentTypes()                 // every registered agent type
val counter = AgentReflection.agentType("CounterAgent") // one by name, or null
```

`AgentTypeInfo` carries the type's `typeName`, `description`, `kind` (`"regular"`/`"http-router"`), `mode` (`"durable"`/`"ephemeral"`), `sourceLanguage`, `constructorParameters`, `methods: List<AgentMethodInfo>` and `implementedBy: ComponentId`. `type.method(name)` finds one method, or `null`.

`AgentMethodInfo` carries `name`, `description`, `parameters: List<AgentParameterInfo>`, `resultWitType: String?` (`null` for no result) and `readOnly: Boolean`. `AgentParameterInfo(name, witType, autoInjected)` — `autoInjected` marks a host-filled parameter such as the caller's `Principal`, which a dynamic caller never supplies; use `method.userParameters` to get only the parameters a caller must pass.

## Call dynamically from an agent ID

```kotlin
val client = AgentReflection.dynamicClient("CounterAgent(\"c1\")") ?: error("unknown agent or type")
try {
    val result = client.invoke("increment")                 // RpcResult
    val byName = client.invoke("setName", mapOf("name" to SchemaValue.Str("Alice")))
} finally {
    client.close()
}
```

`dynamicClient(agentId)` parses the agent ID string, looks up its registered type, and returns a `DynamicAgentClient`, or `null` if the ID does not parse or its type is not registered. `DynamicAgentClient.invoke` has two forms:

- `invoke(method: String, arguments: List<SchemaValue> = emptyList())` — positional, in declaration order (only the user-supplied parameters, not auto-injected ones)
- `invoke(method: String, arguments: Map<String, SchemaValue>)` — by parameter name

Both validate the method name and argument count/names against the agent type's declaration, throwing `DynamicCallException` on a mismatch (unknown method, wrong argument count, missing or unknown named argument). The call itself goes through `WasmRpc`, so the same cautions as `golem-call-another-agent-kotlin` apply: calling yourself or forming a cycle deadlocks, and the client must be `close()`d.

## Building a dynamic client directly

A `DynamicAgentClient` only wraps a `WasmRpc` plus the discovered `AgentTypeInfo`: if you already know the type name and have packed `SchemaValue` constructor arguments some other way (not via `dynamicClient(agentId)`), use `WasmRpc` directly — see `golem-call-another-agent-kotlin`'s "Low-level `WasmRpc`" section — and look up `AgentReflection.agentType(name)` yourself to validate method names and argument shapes before calling.

## When to use this vs a typed or generated client

- A `@RemoteAgent` interface (typed client) or a generated cross-language client is preferred whenever the target agent type is known at compile time — it gives you compile-time checked method signatures.
- Use agent reflection when the target type is selected at runtime (a type name from configuration, a user request, or another agent's metadata), or when building a generic tool that must call agents it was not written against.

## Related skills

- `golem-agent-reflection` — the cross-SDK comparison of normal RPC, caller-defined static, discovered and fully dynamic clients. Kotlin implements the discovered and fully dynamic approaches described there; it has no caller-defined static client.
- `golem-call-another-agent-kotlin` — typed, generated and low-level `WasmRpc` clients
- `golem-get-agent-metadata`
