# Secrets

> Revealing a `secret` resource handle back to its inner typed value, through the capability-gated
> `golem:secrets/reveal@0.1.0` interface. **Status:** Complete.

## Overview

A **secret** is an unforgeable handle to sensitive material held by the Golem runtime — opaque to
guest code. Secrets arrive as [`SchemaValue.SecretVal`](types.md) inside agent inputs and can be
passed around through schema values without ever exposing plaintext.

`reveal` is the **capability-gated escape hatch** that converts a secret back to its inner value.
The capability *is the import*: a component that does not import `golem:secrets/reveal` cannot
reveal secrets at all. Every successful reveal is recorded in the calling agent's oplog as
`(calling-agent, secret-id, timestamp)` (the plaintext is never part of the audit record).

> Prefer **host-mediated substitution** where a host capability accepts `borrow<secret>` directly
> (HTTP auth headers, signing, encryption) — the runtime substitutes plaintext at the syscall
> boundary, so it never crosses into guest memory. `reveal` is the loud-by-design fallback for
> genuinely custom protocols the host doesn't natively support.

## API reference

`cloud.golem.runtime.host.SecretApi`:

```kotlin
object SecretApi {
    /** Reveal a secret value to its inner [witType]-typed value. */
    fun reveal(secret: SchemaValue.SecretVal, witType: String): SchemaValue

    /** Reveal a raw `secret` resource handle. */
    fun reveal(secretHandle: Int, witType: String): SchemaValue
}
```

`witType` is the same rich WIT type-string grammar the [agent surface](types.md) uses — a primitive
(`"string"`, `"s64"`, …) or an arbitrarily nested composite (`"record<user:string,token:string>"`,
`"list<string>"`, …). The host validates it against the secret's pinned inner type and returns the
stored value, which `reveal` lifts into the matching [`SchemaValue`](types.md).

The secret is **borrowed** — the caller keeps ownership of the handle and should release it with
`dropSecret(handle)` when done.

On failure the host returns a `SecretError`, which `reveal` throws wrapped in a
`SecretRevealException`:

```kotlin
sealed class SecretError {
    data class Unavailable(val message: String) : SecretError()       // resolution failed (store gone / partitioned)
    data class VersionNotFound(val versionBytes: List<UByte>) : SecretError() // pinned version destroyed
    data class Internal(val message: String) : SecretError()          // opaque runtime error
}

class SecretRevealException(val error: SecretError) : RuntimeException(...)
```

## Examples

### Reveal a string secret

```kotlin
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.dropSecret
import cloud.golem.runtime.host.SecretApi
import cloud.golem.runtime.host.SecretRevealException

fun useApiKey(secret: SchemaValue.SecretVal): String {
    try {
        val revealed = SecretApi.reveal(secret, "string") as SchemaValue.Str
        return revealed.v
    } catch (e: SecretRevealException) {
        error("could not reveal API key: ${e.error}")
    } finally {
        dropSecret(secret.handle) // borrowed handle: release it when done
    }
}
```

### Reveal a composite secret

```kotlin
// A secret whose inner type is record<user:string,token:string>.
val creds = SecretApi.reveal(secret, "record<user:string,token:string>") as SchemaValue.Record
val user  = (creds.fields[0] as SchemaValue.Str).v
val token = (creds.fields[1] as SchemaValue.Str).v
```

## Notes

- **Composite inner types are fully supported** — `reveal` builds the `expected` schema-graph from
  `witType` (using the shared schema-graph builder) and lifts the returned `schema-value-tree`
  against it, so records, variants, lists, maps, etc. all work.
- The import is the capability; nothing to configure beyond the SDK depending on the
  `golem:secrets/reveal@0.1.0` interface (declared in the native world).
- `reveal` does **not** drop the secret handle (it borrows). Manage the handle's lifetime yourself.
- See [types.md](types.md) for the `SchemaValue` model and the WIT type-string grammar, and the
  [SDK overview](../../README.md).
