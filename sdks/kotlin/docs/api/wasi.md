# WASI Capabilities

> Native Kotlin/Wasm bindings to the WASI capabilities Golem hosts:
> `wasi:keyvalue@0.1.0`, `wasi:blobstore`, `wasi:config@0.2.0-draft`, `wasi:logging`,
> and `wasi:cli/environment@0.2.3`. **Status:** 🟢 Complete.

## Overview

These bindings live in `cloud.golem.runtime.wasi` and give an [`@Agent`](agent-model.md)
direct, JavaScript-free access to the WASI host capabilities Golem provides. Each is a thin
wrapper over raw `@WasmImport` bindings whose canonical-ABI signatures were verified against
the WIT under `wit-native/deps/`, and each mirrors the surface of its Scala-SDK counterpart.

Fallible operations return the SDK's local [`Either`](transactions.md) type
(`cloud.golem.runtime.Either`) — `Either.Left(error)` or `Either.Right(value)`, each with a
`.value` property. Resource-backed capabilities (KeyValue's `Bucket`, Blobstore's
`Container`) hand back a handle wrapper that **must** be `close()`d when done, because the
underlying host handle is not tied to Kotlin/Wasm garbage collection.

| Capability | WIT package | Entry point |
|------------|-------------|-------------|
| KeyValue   | `wasi:keyvalue@0.1.0` (eventual + eventual-batch) | `Bucket.open(...)` |
| Blobstore  | `wasi:blobstore` (unversioned) | `Blobstore` object |
| Config     | `wasi:config@0.2.0-draft` (store) | `Config` object |
| Logging    | `wasi:logging` (unversioned) | `Logging` object |
| Environment| `wasi:cli/environment@0.2.3` | `Environment` object |

For the SDK overview and build/deploy flow see [`../../README.md`](../../README.md).

## KeyValue

`wasi:keyvalue@0.1.0`, the `types` / `eventual` / `eventual-batch` interfaces (the same subset
the Scala SDK wraps; the `atomic` / `cache` / `handle-watch` interfaces are not covered). A
`Bucket` is a named collection of key → `ByteArray` pairs.

Errors are the resource-backed `wasi:keyvalue` error, resolved eagerly to its trace message at
the point of failure and surfaced as a `KvError`:

```kotlin
class KvError internal constructor(handle: Int) {
    val message: String
}

class Bucket internal constructor(private val handle: Int) {
    fun get(key: String): Either<KvError, ByteArray?>
    fun set(key: String, value: ByteArray): Either<KvError, Unit>
    fun delete(key: String): Either<KvError, Unit>
    fun exists(key: String): Either<KvError, Boolean>
    fun keys(): Either<KvError, List<String>>
    fun getMany(keys: List<String>): Either<KvError, List<ByteArray?>>
    fun deleteMany(keys: List<String>): Either<KvError, Unit>
    fun close()

    companion object {
        fun open(name: String): Either<KvError, Bucket>
    }
}
```

`get` / `getMany` return `null` for a missing key (`getMany` returns one nullable entry per
requested key, in order).

## Blobstore

`wasi:blobstore` (unversioned package), the `blobstore` / `container` / `types` interfaces. A
`Container` is a named collection of binary objects. Here the error type is the package's plain
`type error = string`, so every result is `Either<String, T>`.

```kotlin
data class ContainerMetadata(val name: String, val createdAt: Long)
data class ObjectMetadata(val name: String, val container: String, val createdAt: Long, val size: Long)
data class ObjectId(val container: String, val name: String)

object Blobstore {
    fun createContainer(name: String): Either<String, Container>
    fun getContainer(name: String): Either<String, Container>
    fun deleteContainer(name: String): Either<String, Unit>
    fun containerExists(name: String): Either<String, Boolean>
    fun copyObject(src: ObjectId, dest: ObjectId): Either<String, Unit>
    fun moveObject(src: ObjectId, dest: ObjectId): Either<String, Unit>
}

class Container internal constructor(private val handle: Int) {
    fun name(): Either<String, String>
    fun info(): Either<String, ContainerMetadata>
    fun getData(objectName: String, start: Long, end: Long): Either<String, ByteArray>
    fun writeData(objectName: String, data: ByteArray): Either<String, Unit>
    fun listObjects(): Either<String, List<String>>
    fun deleteObject(name: String): Either<String, Unit>
    fun deleteObjects(names: List<String>): Either<String, Unit>
    fun hasObject(name: String): Either<String, Boolean>
    fun objectInfo(name: String): Either<String, ObjectMetadata>
    fun clear(): Either<String, Unit>
    fun close()
}
```

> **Note:** `getData` reads a byte range `[start, end)` of an object. `listObjects` reads a
> single batch of up to 1000 object names and does not paginate further — the same limitation
> as the Scala reference SDK; it is not a full listing for containers with more than 1000
> objects.

## Config

`wasi:config@0.2.0-draft`, the `store` interface. Read-only access to string configuration
values. The `-draft` pre-release suffix is part of the package version.

```kotlin
sealed class ConfigError {
    data class Upstream(val message: String) : ConfigError()
    data class Io(val message: String) : ConfigError()
}

object Config {
    /** `Right(null)` if the key is not found. */
    fun get(key: String): Either<ConfigError, String?>
    fun getAll(): Either<ConfigError, Map<String, String>>
}
```

## Logging

`wasi:logging` (unversioned package), the `logging` interface. Emits a structured log record
(level + context + message) to the host.

```kotlin
enum class LogLevel { TRACE, DEBUG, INFO, WARN, ERROR, CRITICAL }

object Logging {
    fun log(level: LogLevel, context: String, message: String)

    fun trace(message: String, context: String = "")
    fun debug(message: String, context: String = "")
    fun info(message: String, context: String = "")
    fun warn(message: String, context: String = "")
    fun error(message: String, context: String = "")
    fun critical(message: String, context: String = "")
}
```

## Environment

`wasi:cli/environment@0.2.3`. Read the process's environment variables, arguments, and initial
working directory.

```kotlin
object Environment {
    fun getEnvironment(): Map<String, String>
    fun getArguments(): List<String>
    fun initialCwd(): String?
}
```

## Examples

Storing and reading a value in a KeyValue bucket from inside an agent:

```kotlin
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.runtime.Either
import cloud.golem.runtime.wasi.Bucket
import cloud.golem.runtime.wasi.Logging

@Agent
class SessionAgent {

    @Endpoint(put = "/session/{key}")
    fun remember(key: String, value: String) {
        when (val bucket = Bucket.open("sessions")) {
            is Either.Left -> Logging.error("open failed: ${bucket.value.message}")
            is Either.Right -> {
                val b = bucket.value
                b.set(key, value.encodeToByteArray())
                b.close()
            }
        }
    }

    @Endpoint(get = "/session/{key}")
    fun recall(key: String): String? {
        val bucket = (Bucket.open("sessions") as? Either.Right)?.value ?: return null
        val result = bucket.get(key)
        bucket.close()
        return (result as? Either.Right)?.value?.decodeToString()
    }
}
```

Putting and getting a blob:

```kotlin
import cloud.golem.runtime.Either
import cloud.golem.runtime.wasi.Blobstore

fun archive(report: ByteArray): Boolean {
    val container = when (val c = Blobstore.getContainer("reports")) {
        is Either.Right -> c.value
        is Either.Left -> return false
    }
    try {
        container.writeData("2026-07/summary.bin", report)
        return container.hasObject("2026-07/summary.bin") is Either.Right
    } finally {
        container.close()
    }
}
```

Reading config, logging, and inspecting the environment:

```kotlin
import cloud.golem.runtime.Either
import cloud.golem.runtime.wasi.Config
import cloud.golem.runtime.wasi.Environment
import cloud.golem.runtime.wasi.LogLevel
import cloud.golem.runtime.wasi.Logging

fun greeting(): String {
    val name = when (val r = Config.get("greeting-name")) {
        is Either.Right -> r.value ?: "world"
        is Either.Left -> "world"
    }
    Logging.log(LogLevel.INFO, "startup", "cwd=${Environment.initialCwd()}")
    Logging.info("resolved greeting name: $name")
    return "hello, $name"
}
```

## Notes

- **Close your handles.** `Bucket` and `Container` wrap host resources released only by
  `close()`; a `try { ... } finally { handle.close() }` block is the idiomatic pattern.
- **Error shapes differ per capability.** KeyValue surfaces a `KvError` (with `.message`),
  Blobstore uses a plain `String`, and Config uses the `ConfigError` sealed class — check each
  section's signatures rather than assuming a uniform error type.
- **Config and Logging are `object`s**, so no handle bookkeeping is required; likewise
  `Environment` and the `Blobstore` top-level object.
- These bindings match the scope of the Scala SDK's equivalents — where a capability omits an
  interface (KeyValue's `atomic`/`cache`, Blobstore pagination beyond one batch), that mirrors
  the Scala reference rather than being an oversight.

See also: [Agent model](agent-model.md) · [Tools](tools.md) · [Middleware](middleware.md) ·
[Host API](host-api.md) · [SDK README](../../README.md).
