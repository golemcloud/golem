# Agent configuration

> Typed, per-agent configuration with secret leaves (`golem:agent/host.get-config-value`, `agent-type.config`,
> `golem:secrets`). **Status:** ✅ Complete for the 1.6.0 API. Verified on a real 1.6.0 executor with a fixture covering local
> values, nested paths, secrets, optional keys and RPC overrides (`scripts/config-agents/ConfigAgents.kt`).

## Overview

An agent declares its configuration as a **tree of data classes** and takes it as a constructor parameter of type
`Config<T>`. The leaves are ordinary values or `Secret<T>`. KSP derives the agent type's config declarations from the tree and
generates the loader, so the host knows which keys exist (and the deployment can provide them) before the agent runs.

```kotlin
data class DbConfig(val host: String, val port: Int, val password: Secret<String>)

data class AppConfig(
    val appName: String,
    val apiKey: Secret<String>,
    val db: DbConfig,
    val retries: Int?,            // optional: reads as null when not configured
    val tags: List<String>?,
)

@Agent(mount = "/config/{id}")
class ConfigAgent(val id: String, val config: Config<AppConfig>) : BaseAgent() {
    @Endpoint(post = "/greet")
    fun greet(): String {
        val cfg = config.value
        return "Hello from ${cfg.appName}! DB at ${cfg.db.host}:${cfg.db.port}"
    }
}
```

## The tree

- A **non-null data class** contributes one path segment per field and is never a leaf (`db.host`, `db.port`, `db.password`).
- Every other type the SDK maps (primitives, lists, enums, `Either`, …) and every nullable type is a **local leaf**
  (a nullable data class is a leaf, read as one value).
- `Secret<T>` is a **secret leaf** (`T` is the revealed value's type).
- Paths are the field names. The `Config` parameter is not a constructor argument: it keeps its position in the constructor but
  takes no input value, so callers (RPC, the CLI) never pass it.

`config.value` reads the current configuration every time it is accessed, so shared keys that change while the agent lives are
picked up. Reading an undeclared key fails, except an `option` (nullable) key, which reads as `null`.

## Where values come from

- **Local** leaves come from the component's per-agent-type config (`golem.yaml` / deployment) plus overrides given when the
  agent was created.
- **Secret** leaves come from the environment's **agent secrets** (same path). A required secret that is missing or has the wrong
  type stops the agent from being created.

## Secrets

```kotlin
class Secret<T> {
    fun get(): T                       // reveals the value (recorded in the agent's oplog)
    fun handle(): SecretHandle         // an owned handle, without revealing
}

class SecretHandle {
    fun id(): List<UByte>              // stable identity
    fun metadata(): SecretMetadata     // configKey, version, resolvedAt, category (never the value)
    fun release()
}
```

Prefer passing a `SecretHandle` (`intoSchemaValue()`) to host capabilities that accept secrets over revealing the value; see
[Secrets](secrets.md) for `SecretApi.reveal` and the `secret` schema type.

## Config overrides on RPC

Creating an agent through RPC can override its local config keys. Typed clients take the overrides as a constructor argument:

```kotlin
@RemoteAgent(typeName = "ConfigAgent")
interface ConfigAgentClient { fun greet(): String }

val client = ConfigAgentClientRpc(
    SchemaValue.Record(listOf(SchemaValue.Str("id"))),
    listOf(
        ConfigOverride(listOf("appName"), TypedSchemaValue("string", SchemaValue.Str("OverriddenApp"))),
        ConfigOverride(listOf("db", "port"), TypedSchemaValue("s32", SchemaValue.S32(9999))),
    ),
)
```

`WasmRpc(agentTypeName, constructorArgs, config)` takes the same list. Only keys the target agent type declares are accepted.

## Notes

- `Config` and `Secret` have public constructors only because generated code calls them; user code gets them from the agent's
  constructor.
- `secret<T>` is also a WIT-type string in the SDK's type grammar (a `secret` handle whose revealed value has type `T`).
- Not covered: typed per-field override builders (overrides are a list of `ConfigOverride`), config in tools/middleware, `Config<T>`
  on `@RemoteAgent` interfaces.

See also: [Secrets](secrets.md) · [RPC](rpc.md) · [Agent model](agent-model.md) · [SDK README](../../README.md).
