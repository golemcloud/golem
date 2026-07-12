@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.host

import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.collectTypeNodes
import cloud.golem.runtime.lowerSchemaGraphInto
import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftSingleValue
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.storeInt

// golem:secrets/reveal@0.1.0 -- the capability-gated escape hatch that unpacks a `secret` resource
// to its inner typed value. reveal(s: borrow<secret>, expected: schema-graph) ->
// result<schema-value-tree, secret-error>. The schema-graph is flattened into params (verified via
// abi-dump `sig`): [secret, type-nodes.ptr, type-nodes.len, defs.ptr, defs.len, root, retptr].
@kotlin.wasm.WasmImport("golem:secrets/reveal@0.1.0", "reveal")
private external fun hostSecretReveal(
    secret: Int,
    typeNodesPtr: Int,
    typeNodesLen: Int,
    defsPtr: Int,
    defsLen: Int,
    root: Int,
    retPtr: Int,
)

// schema-graph: 20B { type-nodes list @0 (ptr@0,len@4), defs list @8 (ptr@8,len@12), root @16 }.
private const val SG_SIZE = 20
private const val SG_ROOT = 16

/** The error arm of `reveal`'s `result<schema-value-tree, secret-error>` (golem:secrets@0.1.0). */
sealed class SecretError {
    /** The secret was bound but its current resolution failed (store entry deleted / partitioned). */
    data class Unavailable(val message: String) : SecretError()

    /** The pinned version no longer exists; carries `secret-version.bytes`. */
    data class VersionNotFound(val versionBytes: List<UByte>) : SecretError()

    /** Internal runtime error (opaque message, never plaintext). */
    data class Internal(val message: String) : SecretError()
}

/** Thrown by [SecretApi.reveal] when the host returns a [SecretError]. */
class SecretRevealException(val error: SecretError) : RuntimeException("secret reveal failed: $error")

// Decodes a `secret-error` at [base] (tag@0, payload@4). Cases (verified against types.wit):
// 0 unavailable(string), 1 version-not-found(secret-version{bytes: list<u8>}), 2 internal(string).
// string and list<u8> share the (ptr@base+4, len@base+8) shape.
internal fun liftSecretError(base: Int): SecretError {
    val ptr = loadInt(base + 4)
    val len = loadInt(base + 8)
    return when (val tag = loadByte(base).toInt() and 0xFF) {
        0 -> SecretError.Unavailable(liftString(ptr, len))
        1 -> SecretError.VersionNotFound((0 until len).map { loadByte(ptr + it).toUByte() })
        2 -> SecretError.Internal(liftString(ptr, len))
        else -> SecretError.Internal("unknown secret-error tag=$tag")
    }
}

/**
 * Native access to `golem:secrets/reveal@0.1.0`. The single capability the interface grants:
 * [reveal] unpacks a `secret` resource handle back to its inner plaintext value, as a
 * [SchemaValue] of the caller-declared type. The import itself IS the capability -- a component
 * that does not import this interface cannot reveal secrets -- and every successful reveal is
 * recorded in the calling agent's oplog. Prefer host-mediated substitution (host capabilities
 * taking `borrow<secret>` directly) where available; reveal is the loud-by-design fallback for
 * genuinely custom protocols the host doesn't natively support.
 *
 * Mirrors the Scala SDK's `golem.host.SecretApi.reveal`.
 */
object SecretApi {

    /** Reveals [secret] as its inner value, expected to have type [witType]. See [reveal]. */
    fun reveal(secret: SchemaValue.SecretVal, witType: String): SchemaValue = reveal(secret.handle, witType)

    /**
     * Reveals the `secret` resource [secretHandle] as its inner value, expected to have type
     * [witType] -- the same rich witType-string grammar the agent surface uses (a primitive such
     * as `"string"`, or a composite such as `"record<user:string,token:string>"`). The host
     * validates [witType] against the secret's pinned inner type and returns the stored value.
     *
     * `secret` is *borrowed*: the caller keeps ownership of the handle (drop it via `dropSecret`
     * when done). Throws [SecretRevealException] if the host returns a [SecretError].
     */
    fun reveal(secretHandle: Int, witType: String): SchemaValue {
        // Build `expected`: a schema-graph whose semantic root is `witType`. collectTypeNodes
        // registers the root type last, so its index is typeIndex[witType]; lowerSchemaGraphInto
        // leaves root=0 (a placeholder valid only for the agent-type), so set it explicitly.
        val typeIndex = collectTypeNodes(listOf(witType))
        val graph = alloc(SG_SIZE, 4)
        lowerSchemaGraphInto(graph, 0, typeIndex)
        storeInt(graph + SG_ROOT, typeIndex.getValue(witType))

        val ret = alloc(16, 4) // result<schema-value-tree, secret-error>: tag@0, payload@4
        hostSecretReveal(
            secretHandle,
            loadInt(graph),
            loadInt(graph + 4), // type-nodes ptr/len
            loadInt(graph + 8),
            loadInt(graph + 12), // defs ptr/len
            loadInt(graph + SG_ROOT), // root
            ret,
        )
        if (loadByte(ret).toInt() and 0xFF == 0) {
            // ok: schema-value-tree inline @ ret+4 (value-nodes.ptr@4, len@8, root@12).
            return liftSingleValue(ret + 4, witType)
        }
        throw SecretRevealException(liftSecretError(ret + 4))
    }
}
