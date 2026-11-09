@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.config

import cloud.golem.permissions.Timestamp
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.TypedSchemaValue
import cloud.golem.runtime.collectTypeNodes
import cloud.golem.runtime.dropSecret
import cloud.golem.runtime.host.SecretApi
import cloud.golem.runtime.lowerSchemaGraphInto
import cloud.golem.runtime.lowerStringToPtrLen
import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftSingleValue
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeInt

// golem:agent/host@2.0.0 get-config-value(key: list<string>, expected: schema-graph) -> result<schema-value-tree, config-value-error>.
// Flat params: [key.ptr, key.len, type-nodes.ptr, type-nodes.len, defs.ptr, defs.len, root, retptr]. The result is 16 B:
// tag@0, ok = schema-value-tree @4 (value-nodes ptr@4/len@8, root@12), err = config-value-error (permission-denied) @4.
@kotlin.wasm.WasmImport("golem:agent/host@2.0.0", "get-config-value")
private external fun hostGetConfigValue(
    keyPtr: Int,
    keyLen: Int,
    typeNodesPtr: Int,
    typeNodesLen: Int,
    defsPtr: Int,
    defsLen: Int,
    root: Int,
    retPtr: Int,
)

// golem:secrets/types@0.1.0: id(borrow<secret>) -> secret-id {bytes: list<u8>} (8 B); metadata(borrow<secret>) -> secret-metadata (56 B, align 8).
@kotlin.wasm.WasmImport("golem:secrets/types@0.1.0", "id")
private external fun hostSecretId(secret: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:secrets/types@0.1.0", "metadata")
private external fun hostSecretMetadata(secret: Int, retPtr: Int)

private const val SG_SIZE = 20
private const val SG_ROOT = 16

/**
 * Overrides one config key of an agent being created through RPC (`wasm-rpc.create`'s `agent-config`): the value is a
 * self-describing [TypedSchemaValue]. Only keys the target agent type declares are accepted.
 */
data class ConfigOverride(val path: List<String>, val value: TypedSchemaValue)

/** Thrown when the host refuses to hand out a config value. */
class ConfigException(message: String) : RuntimeException(message)

/**
 * The agent's configuration: a tree of data classes whose leaves are ordinary values or [Secret]s. Take it as a constructor
 * parameter (`class MyAgent(val config: Config<AppConfig>)`); KSP derives the config declarations from `AppConfig` and generates
 * the loader. [value] reads the current configuration each time it is accessed (shared keys can change while the agent lives).
 *
 * The constructor is public only because generated code calls it.
 */
class Config<T>(private val load: () -> T) {
    val value: T get() = load()
}

/**
 * A secret leaf of the config tree. [get] reveals the secret's value (recorded in the agent's oplog); [handle] gives an owned
 * handle without revealing it. Constructed by generated code.
 */
class Secret<T>(
    private val path: List<String>,
    private val innerWitType: String,
    private val decode: (SchemaValue) -> T,
) {
    /** Reveals the secret and returns its value. */
    fun get(): T {
        val handle = ConfigRuntime.secretHandle(path, innerWitType)
        try {
            return decode(SecretApi.reveal(handle, innerWitType))
        } finally {
            dropSecret(handle)
        }
    }

    /** Acquires an owned handle to the secret without revealing it. [SecretHandle.release] it when done. */
    fun handle(): SecretHandle = SecretHandle(ConfigRuntime.secretHandle(path, innerWitType))
}

/** What the runtime knows about a secret without revealing its value. */
class SecretMetadata(
    /** The config key the secret was resolved from, if it came from config. */
    val configKey: List<String>?,
    /** The pinned version, if the secret is versioned. */
    val version: List<UByte>?,
    /** When the secret was resolved. */
    val resolvedAt: Timestamp,
    val category: String?,
)

/** An owned `secret` resource handle. Call [release] when done (handles are not reclaimed by Kotlin/Wasm GC). */
class SecretHandle internal constructor(private var handle: Int) {
    private fun live(): Int {
        check(handle != 0) { "this secret handle was released" }
        return handle
    }

    /** The raw handle, to BORROW it for one host call (the handle stays owned by this object). */
    internal fun borrowHandle(): Int = live()

    /** The secret's stable identity (opaque bytes). */
    fun id(): List<UByte> {
        val ret = alloc(8, 4)
        hostSecretId(live(), ret)
        val ptr = loadInt(ret)
        return List(loadInt(ret + 4)) { loadByte(ptr + it).toUByte() }
    }

    /** Metadata about the secret (never its value). */
    fun metadata(): SecretMetadata {
        val ret = alloc(56, 8)
        hostSecretMetadata(live(), ret)
        // secret-metadata: config-key: option<list<string>>@0 (tag@0, list@4), version: option<secret-version>@12 (tag@12, bytes list@16),
        // resolved-at: {seconds: s64, nanoseconds: u32}@24, category: option<string>@40 (tag@40, string@44).
        val configKey = if (loadByte(ret).toInt() and 0xFF == 0) {
            null
        } else {
            val ptr = loadInt(ret + 4)
            List(loadInt(ret + 8)) { liftString(loadInt(ptr + it * 8), loadInt(ptr + it * 8 + 4)) }
        }
        val version = if (loadByte(ret + 12).toInt() and 0xFF == 0) {
            null
        } else {
            val ptr = loadInt(ret + 16)
            List(loadInt(ret + 20)) { loadByte(ptr + it).toUByte() }
        }
        val resolvedAt = Timestamp(loadLong(ret + 24), loadInt(ret + 32))
        val category = if (loadByte(ret + 40).toInt() and 0xFF == 0) null else liftString(loadInt(ret + 44), loadInt(ret + 48))
        return SecretMetadata(configKey, version, resolvedAt, category)
    }

    /** Hands the handle to the host (an argument or result that takes ownership). The handle is no longer usable here. */
    fun intoSchemaValue(): SchemaValue.SecretVal {
        val h = live()
        handle = 0
        return SchemaValue.SecretVal(h)
    }

    /** Drops the handle. */
    fun release() {
        if (handle != 0) {
            dropSecret(handle)
            handle = 0
        }
    }
}

/** Reads config leaves from the host. Used by generated config loaders. */
object ConfigRuntime {
    /**
     * Reads the config value at [path] as a value of WIT type [witType] (`option<..>` types read as `none` when the key is
     * undeclared; any other undeclared key fails).
     * @throws ConfigException if the host refuses the key.
     */
    fun local(path: List<String>, witType: String): SchemaValue {
        val ret = fetch(path, witType)
        return liftSingleValue(ret + 4, witType)
    }

    /** Reads the secret at [path] (declared as `secret<innerWitType>`) and returns the owned secret handle. */
    fun secretHandle(path: List<String>, innerWitType: String): Int {
        val secretWit = "secret<$innerWitType>"
        val ret = fetch(path, secretWit)
        return (liftSingleValue(ret + 4, secretWit) as SchemaValue.SecretVal).handle
    }

    private fun fetch(path: List<String>, witType: String): Int {
        // The expected graph's semantic root is the leaf's own type (collectTypeNodes registers the root last).
        val typeIndex = collectTypeNodes(listOf(witType))
        val graph = alloc(SG_SIZE, 4)
        lowerSchemaGraphInto(graph, 0, typeIndex)
        storeInt(graph + SG_ROOT, typeIndex.getValue(witType))
        val keys = if (path.isEmpty()) 0 else alloc(path.size * 8, 4)
        path.forEachIndexed { i, segment ->
            val (p, l) = lowerStringToPtrLen(segment)
            storeInt(keys + i * 8, p)
            storeInt(keys + i * 8 + 4, l)
        }
        val ret = alloc(16, 4)
        hostGetConfigValue(keys, path.size, loadInt(graph), loadInt(graph + 4), loadInt(graph + 8), loadInt(graph + 12), loadInt(graph + SG_ROOT), ret)
        if (loadByte(ret).toInt() and 0xFF != 0) throw ConfigException("the host denied access to config key ${path.joinToString(".")}")
        return ret
    }
}
