@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.wasi

import cloud.golem.wasm.alloc
import cloud.golem.wasm.storeByte

// Raw canonical-ABI import binding to wasi:logging/logging (package `wasi:logging`, unversioned
// -- confirmed via wit-native/deps/logging/logging.wit's `package wasi:logging;` declaration,
// matching the Scala facade's own `@JSImport("wasi:logging/logging", ...)` exactly, no @version
// suffix). Signature verified via abi-dump's `sig` mode.
@kotlin.wasm.WasmImport("wasi:logging/logging", "log")
private external fun hostLog(level: Int, contextPtr: Int, contextLen: Int, messagePtr: Int, messageLen: Int)

/** Matches `wasi:logging/logging`'s `level` enum case order exactly. */
enum class LogLevel { TRACE, DEBUG, INFO, WARN, ERROR, CRITICAL }

private fun lowerStringToPtrLen(s: String): Pair<Int, Int> {
    val bytes = s.encodeToByteArray()
    val ptr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(ptr + i, bytes[i])
    return ptr to bytes.size
}

/** Native SDK access to WASI logging (`wasi:logging/logging`). Mirrors the Scala SDK's `Logging` object. */
object Logging {
    fun log(level: LogLevel, context: String, message: String) {
        val (contextPtr, contextLen) = lowerStringToPtrLen(context)
        val (messagePtr, messageLen) = lowerStringToPtrLen(message)
        hostLog(level.ordinal, contextPtr, contextLen, messagePtr, messageLen)
    }

    fun trace(message: String, context: String = "") = log(LogLevel.TRACE, context, message)
    fun debug(message: String, context: String = "") = log(LogLevel.DEBUG, context, message)
    fun info(message: String, context: String = "") = log(LogLevel.INFO, context, message)
    fun warn(message: String, context: String = "") = log(LogLevel.WARN, context, message)
    fun error(message: String, context: String = "") = log(LogLevel.ERROR, context, message)
    fun critical(message: String, context: String = "") = log(LogLevel.CRITICAL, context, message)
}
