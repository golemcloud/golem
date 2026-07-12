@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.wasi

import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt

// Raw canonical-ABI import bindings to wasi:cli/environment@0.2.3. Signatures verified via
// abi-dump's `sig` mode against wit-native/deps/cli/environment.wit.
@kotlin.wasm.WasmImport("wasi:cli/environment@0.2.3", "get-environment")
private external fun hostGetEnvironment(retPtr: Int)

@kotlin.wasm.WasmImport("wasi:cli/environment@0.2.3", "get-arguments")
private external fun hostGetArguments(retPtr: Int)

@kotlin.wasm.WasmImport("wasi:cli/environment@0.2.3", "initial-cwd")
private external fun hostInitialCwd(retPtr: Int)

private fun liftListOfString(base: Int): List<String> {
    val dataPtr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i ->
        val elemPtr = dataPtr + i * 8
        liftString(loadInt(elemPtr), loadInt(elemPtr + 4))
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

/** Native SDK access to `wasi:cli/environment@0.2.3`. Mirrors the Scala SDK's `Environment` object. */
object Environment {
    /** The POSIX-style environment variables. */
    fun getEnvironment(): Map<String, String> {
        val retPtr = alloc(8, 4) // list<tuple<string,string>>: {ptr: i32, len: i32}
        hostGetEnvironment(retPtr)
        return liftListOfStringPair(retPtr).toMap()
    }

    /** The POSIX-style arguments to the program. */
    fun getArguments(): List<String> {
        val retPtr = alloc(8, 4) // list<string>: {ptr: i32, len: i32}
        hostGetArguments(retPtr)
        return liftListOfString(retPtr)
    }

    /** A path programs should use as their initial current working directory, if any. */
    fun initialCwd(): String? {
        val retPtr = alloc(12, 4) // option<string>: tag@0(1,1), payload@4(8,4)
        hostInitialCwd(retPtr)
        return if (loadByte(retPtr).toInt() == 0) null else liftString(loadInt(retPtr + 4), loadInt(retPtr + 8))
    }
}
