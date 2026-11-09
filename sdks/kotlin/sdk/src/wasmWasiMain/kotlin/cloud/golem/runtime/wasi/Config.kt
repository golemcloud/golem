@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.wasi

import cloud.golem.runtime.Either
import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.storeByte

// Raw canonical-ABI import bindings to wasi:config/store@0.2.0-draft. Package version carries a
// "-draft" pre-release suffix (confirmed in wit-native/deps/config/world.wit's
// `package wasi:config@0.2.0-draft;`) -- included verbatim in the raw module string, matching
// the Scala facade's own `@JSImport("wasi:config/store@0.2.0-draft", ...)` exactly. Signatures
// verified via abi-dump's `sig` mode.
@kotlin.wasm.WasmImport("wasi:config/store@0.2.0-draft", "get")
private external fun hostGet(keyPtr: Int, keyLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:config/store@0.2.0-draft", "get-all")
private external fun hostGetAll(retPtr: Int)

/** Matches `wasi:config/store@0.2.0-draft`'s `error` variant. */
sealed class ConfigError {
    data class Upstream(val message: String) : ConfigError()
    data class Io(val message: String) : ConfigError()
}

// error: size=12 align=4, tag_size=1, payload_offset=4 (both cases: string, 8 bytes).
private fun liftConfigError(base: Int): ConfigError {
    val tag = loadByte(base).toInt() and 0xFF
    val payload = base + 4
    return when (tag) {
        0 -> ConfigError.Upstream(liftString(loadInt(payload), loadInt(payload + 4)))
        1 -> ConfigError.Io(liftString(loadInt(payload), loadInt(payload + 4)))
        else -> error("unknown wasi:config error tag: $tag")
    }
}

private fun liftListOfStringPair(base: Int): List<Pair<String, String>> {
    val dataPtr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i ->
        val elemPtr = dataPtr + i * 16
        liftString(loadInt(elemPtr), loadInt(elemPtr + 4)) to liftString(loadInt(elemPtr + 8), loadInt(elemPtr + 12))
    }
}

private fun lowerStringToPtrLen(s: String): Pair<Int, Int> {
    val bytes = s.encodeToByteArray()
    val ptr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(ptr + i, bytes[i])
    return ptr to bytes.size
}

/** Native SDK access to the WASI config store (`wasi:config/store@0.2.0-draft`). Mirrors the Scala SDK's `Config` object. */
object Config {
    /** A configuration value of type `string` associated with [key]. `Right(null)` if the key is not found. */
    fun get(key: String): Either<ConfigError, String?> {
        val (keyPtr, keyLen) = lowerStringToPtrLen(key)
        // result<option<string>, error>: tag@0(1,1), payload@4 (max(option<string> 12,4, error 12,4) = 12) -> 16 total.
        val retPtr = alloc(16, 4)
        hostGet(keyPtr, keyLen, retPtr)
        return if (loadByte(retPtr).toInt() == 0) {
            val optBase = retPtr + 4
            val value = if (loadByte(optBase).toInt() == 0) null else liftString(loadInt(optBase + 4), loadInt(optBase + 8))
            Either.Right(value)
        } else {
            Either.Left(liftConfigError(retPtr + 4))
        }
    }

    /** Every configuration key-value pair of type `string`. */
    fun getAll(): Either<ConfigError, Map<String, String>> {
        // result<list<tuple<string,string>>, error>: tag@0(1,1), payload@4 (max(list 8,4, error 12,4) = 12) -> 16 total.
        val retPtr = alloc(16, 4)
        hostGetAll(retPtr)
        return if (loadByte(retPtr).toInt() == 0) {
            Either.Right(liftListOfStringPair(retPtr + 4).toMap())
        } else {
            Either.Left(liftConfigError(retPtr + 4))
        }
    }
}
