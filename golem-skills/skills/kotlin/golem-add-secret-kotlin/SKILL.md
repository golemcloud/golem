---
name: golem-add-secret-kotlin
description: "Using secrets in a Kotlin agent. Use when the user needs an API key, password or token from agent secrets, wants to reveal or pass a secret without exposing it, or sets secrets with the CLI."
---

# Using Secrets (Kotlin)

Secrets are opaque handles to sensitive values held by Golem. They arrive through the agent's `Config<T>` tree as `Secret<T>` leaves.

```kotlin
import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.config.Config
import cloud.golem.config.Secret

data class AppConfig(val apiKey: Secret<String>, val label: String)

@Agent(mount = "/svc/{id}")
class Svc(val id: String, val config: Config<AppConfig>) : BaseAgent() {
    @Endpoint(post = "/call")
    fun call(): String {
        val key = config.value.apiKey.get()        // reveals; recorded in the oplog
        return fetchWith(key)
    }
}
```

## Choosing how to use a secret

- `secret.get()` reveals the plaintext. Every reveal is audited (secret id, agent, time; never the value).
- `secret.handle()` returns an owned `cloud.golem.config.SecretHandle` without revealing. Prefer passing the `Secret<String>` itself to host capabilities that accept secrets (for example `DurableStreams.json(url, options, auth = config.value.bearer)`) so the plaintext never enters guest memory. Call `handle.release()` when done; `handle.id()` is the stable identity and `handle.metadata()` gives config key, version, resolved time and category (never the value).
- Low level: `cloud.golem.runtime.host.SecretApi.reveal(secretVal: SchemaValue.SecretVal, witType)` (or `reveal(handleInt, witType)`) reveals a raw secret value; release a borrowed handle with `cloud.golem.runtime.dropSecret(handle)`. Failures throw `SecretRevealException` whose `error` is `SecretError.Unavailable`, `VersionNotFound` or `Internal`. `Secret<T>` itself is only available as a `Config<T>` leaf, not as a method parameter.

## Managing secrets

```shell
golem secret create apiKey --type String --value "sk-abc123"
golem secret create db.password --type String --value "s3cret"
printenv API_KEY | golem secret create apiKey --type String --value-stdin --update-existing
golem secret list
golem secret get apiKey
golem secret update apiKey --value "new-value"
golem secret delete apiKey
```

The secret path is the config path (`apiKey`, `db.password`; segments are normalised to lowerCamelCase). `--type` is a Kotlin type expression (`String`, `Long`, `Boolean`); without `--value` the CLI prompts with hidden input. A required secret that is missing or has the wrong type prevents the agent from being created.

For local development, defaults can be set per environment in `golem.yaml` (not for production, and do not commit real secrets):

```yaml
secretDefaults:
  local:
    apiKey: "dev-key-123"
    db:
      password: "dev-password"
```

## Related skills

- `golem-add-config-kotlin`
- `golem-durable-streams-kotlin`
