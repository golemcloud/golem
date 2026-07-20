@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.host

import cloud.golem.runtime.HostApi
import cloud.golem.runtime.TypedSchemaValue
import cloud.golem.runtime.liftTypedSchemaValue
import cloud.golem.runtime.lowerTypedSchemaValueInto
import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeLong
import cloud.golem.wasm.writeStringField

// Raw canonical-ABI import bindings to golem:durability/durability@1.5.0 (package
// golem:durability@1.5.0, interface "durability"). Signatures verified via
// wit-parser::Resolve::wasm_signature(AbiVariant::GuestImport) against
// wit-native/deps/golem-durability/golem-durability.wit. This interface is not pulled in
// transitively by anything else in wit-native/main.wit's world, so it needed an explicit
// `import golem:durability/durability@1.5.0;` there (same situation as the golem:agent/host@2.0.0 import).
@kotlin.wasm.WasmImport("golem:durability/durability@1.5.0", "observe-function-call")
private external fun hostObserveFunctionCall(ifacePtr: Int, ifaceLen: Int, funcPtr: Int, funcLen: Int)

// begin-durable-function(function-type: durable-function-type) -> oplog-index. durable-function-type
// (an alias of `wrapped-function-type`, a 6-case variant whose only payload shape is
// `option<oplog-index>` i.e. `option<u64>`) flattens to 3 core words: the outer tag, then the
// union of each case's flattened payload -- here just the inner option's [tag, payload]
// pair -- confirmed via abi-dump's `sig` mode against the real signature, not assumed from the
// WIT shape alone.
@kotlin.wasm.WasmImport("golem:durability/durability@1.5.0", "begin-durable-function")
private external fun hostBeginDurableFunction(tag: Int, hasBegin: Int, begin: Long): Long

@kotlin.wasm.WasmImport("golem:durability/durability@1.5.0", "end-durable-function")
private external fun hostEndDurableFunction(tag: Int, hasBegin: Int, begin: Long, beginIndex: Long, forcedCommit: Int)

@kotlin.wasm.WasmImport("golem:durability/durability@1.5.0", "current-durable-execution-state")
private external fun hostCurrentDurableExecutionState(retPtr: Int)

// persist-durable-function-invocation(function-name: string, request: typed-schema-value,
// response: typed-schema-value, function-type: durable-function-type). indirect_params=true
// (the flattened param set exceeds the ABI's core-arg limit), so all four args are bundled into
// one 96-byte, 8-aligned memory block passed by pointer -- layout verified via abi-dump's
// `funcargs` mode against the real WIT (2026-07-10): function-name@0, request@8, response@40,
// function-type@72.
@kotlin.wasm.WasmImport("golem:durability/durability@1.5.0", "persist-durable-function-invocation")
private external fun hostPersistDurableFunctionInvocation(argsPtr: Int)

// read-persisted-durable-function-invocation() -> persisted-durable-function-invocation. retptr=true:
// the guest allocates the 88-byte/align-8 result record and passes its pointer; the host writes
// into it. Result layout verified via abi-dump `resulttype` (2026-07-10): timestamp@0 (datetime,
// 16B), function-name@16 (string), response@24 (typed-schema-value, 32B), function-type@56
// (durable-function-type in-memory variant, 24B), entry-version@80 (enum, 1B).
@kotlin.wasm.WasmImport("golem:durability/durability@1.5.0", "read-persisted-durable-function-invocation")
private external fun hostReadPersistedDurableFunctionInvocation(retPtr: Int)

/**
 * Matches `golem:api/oplog@1.5.0`'s `wrapped-function-type` variant (aliased as
 * `durable-function-type` in `golem:durability@1.5.0`) case order and payload shape exactly.
 */
sealed class DurableFunctionType {
    object ReadLocal : DurableFunctionType()
    object WriteLocal : DurableFunctionType()
    object ReadRemote : DurableFunctionType()
    object WriteRemote : DurableFunctionType()
    data class WriteRemoteBatched(val begin: Long?) : DurableFunctionType()
    data class WriteRemoteTransaction(val begin: Long?) : DurableFunctionType()
}

// Lowers a DurableFunctionType into the 3 flattened core words wit-bindgen expects for this
// variant: (outer tag, inner option<oplog-index> tag, inner option<oplog-index> payload).
private fun lowerDurableFunctionType(type: DurableFunctionType): Triple<Int, Int, Long> = when (type) {
    DurableFunctionType.ReadLocal -> Triple(0, 0, 0L)
    DurableFunctionType.WriteLocal -> Triple(1, 0, 0L)
    DurableFunctionType.ReadRemote -> Triple(2, 0, 0L)
    DurableFunctionType.WriteRemote -> Triple(3, 0, 0L)
    is DurableFunctionType.WriteRemoteBatched -> Triple(4, if (type.begin != null) 1 else 0, type.begin ?: 0L)
    is DurableFunctionType.WriteRemoteTransaction -> Triple(5, if (type.begin != null) 1 else 0, type.begin ?: 0L)
}

// Writes a durable-function-type as an IN-MEMORY variant (24 bytes, align 8) at [base], for use
// inside a bundled args block (as opposed to the flattened form above used for direct call
// args). Layout verified via abi-dump: tag @ 0 (u8), payload @ 8 = option<oplog-index> i.e.
// option<u64> whose own tag is @ 8 and u64 value @ 16.
@OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)
private fun writeDurableFunctionTypeInMemory(base: Int, type: DurableFunctionType) {
    val (tag, hasBegin, begin) = lowerDurableFunctionType(type)
    storeByte(base, tag.toByte())
    storeByte(base + 8, hasBegin.toByte()) // option<oplog-index> discriminant
    if (hasBegin == 1) storeLong(base + 16, begin) // oplog-index (u64)
}

// Inverse of writeDurableFunctionTypeInMemory: reads a 24-byte in-memory durable-function-type.
@OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)
internal fun readDurableFunctionTypeInMemory(base: Int): DurableFunctionType {
    val begin: Long? = if (loadByte(base + 8).toInt() != 0) loadLong(base + 16) else null
    return when (loadByte(base).toInt() and 0xFF) {
        0 -> DurableFunctionType.ReadLocal
        1 -> DurableFunctionType.WriteLocal
        2 -> DurableFunctionType.ReadRemote
        3 -> DurableFunctionType.WriteRemote
        4 -> DurableFunctionType.WriteRemoteBatched(begin)
        5 -> DurableFunctionType.WriteRemoteTransaction(begin)
        else -> error("native readDurableFunctionType: unknown tag")
    }
}

/** WIT `oplog-entry-version` enum (golem:durability@1.5.0). */
enum class OplogEntryVersion { V1, V2 }

/** A persisted durable function invocation, read back during replay. */
data class PersistedDurableFunctionInvocation(
    /** Timestamp seconds since the Unix epoch. */
    val timestampSeconds: Long,
    /** Sub-second nanoseconds of the timestamp. */
    val timestampNanoseconds: Int,
    val functionName: String,
    val response: TypedSchemaValue,
    val functionType: DurableFunctionType,
    val entryVersion: OplogEntryVersion,
)

/** Matches `golem:durability@1.5.0`'s `durable-execution-state` record (2 bytes, align 1) field-for-field. */
data class DurableExecutionState(val isLive: Boolean, val persistenceLevel: HostApi.PersistenceLevel)

/**
 * Native SDK access to `golem:durability/durability@1.5.0`. It provides the
 * core oplog-observation/durable-function-region primitives, plus the current durable
 * execution state. Mirrors the Scala SDK's `DurabilityApi` object
 * (`sdks/scala/core/js/src/main/scala/golem/host/DurabilityApi.scala`) for this subset.
 *
 * `persistDurableFunctionInvocation`/`readPersistedDurableFunctionInvocation` lower/lift the
 * request/response `typed-schema-value`s via `TypedSchemaValue` encode/decode, which supports the
 * full composite grammar (records/variants/enums/lists/options/tuples/maps/results, nested). Still
 * deferred: `lazy-initialized-pollable` (a WIT `resource` for async pollable wiring, with no
 * consumer in the SDK's synchronous model -- part of the deferred wasi:io/poll workstream).
 */
object DurabilityApi {
    fun observeFunctionCall(iface: String, function: String) {
        val ifaceBytes = iface.encodeToByteArray()
        val ifacePtr = alloc(ifaceBytes.size, 1)
        for (i in ifaceBytes.indices) storeByte(ifacePtr + i, ifaceBytes[i])
        val funcBytes = function.encodeToByteArray()
        val funcPtr = alloc(funcBytes.size, 1)
        for (i in funcBytes.indices) storeByte(funcPtr + i, funcBytes[i])
        hostObserveFunctionCall(ifacePtr, ifaceBytes.size, funcPtr, funcBytes.size)
    }

    fun beginDurableFunction(functionType: DurableFunctionType): Long {
        val (tag, hasBegin, begin) = lowerDurableFunctionType(functionType)
        return hostBeginDurableFunction(tag, hasBegin, begin)
    }

    fun endDurableFunction(functionType: DurableFunctionType, beginIndex: Long, forcedCommit: Boolean) {
        val (tag, hasBegin, begin) = lowerDurableFunctionType(functionType)
        hostEndDurableFunction(tag, hasBegin, begin, beginIndex, if (forcedCommit) 1 else 0)
    }

    fun currentDurableExecutionState(): DurableExecutionState {
        val ptr = alloc(2, 1)
        hostCurrentDurableExecutionState(ptr)
        val isLive = loadByte(ptr).toInt() != 0
        val persistenceLevel = HostApi.PersistenceLevel.entries[loadByte(ptr + 1).toInt() and 0xFF]
        return DurableExecutionState(isLive, persistenceLevel)
    }

    /**
     * Writes a durable-function-invocation record to the agent's oplog. [request]/[response] are
     * self-describing `typed-schema-value`s (schema graph + value); any composite [TypedSchemaValue]
     * is supported. Bundles the four args into the 96-byte block the host's indirect-params ABI
     * expects and calls the import.
     */
    fun persistDurableFunctionInvocation(
        functionName: String,
        request: TypedSchemaValue,
        response: TypedSchemaValue,
        functionType: DurableFunctionType,
    ) {
        val args = alloc(96, 8)
        writeStringField(args, 0, functionName) // function-name @ 0 (string, 8B)
        lowerTypedSchemaValueInto(args + 8, request) // request @ 8 (typed-schema-value, 32B)
        lowerTypedSchemaValueInto(args + 40, response) // response @ 40 (typed-schema-value, 32B)
        writeDurableFunctionTypeInMemory(args + 72, functionType) // function-type @ 72 (24B)
        hostPersistDurableFunctionInvocation(args)
    }

    /**
     * Reads the next persisted durable-function invocation from the oplog during replay. The
     * [PersistedDurableFunctionInvocation.response] is decoded via the typed-schema-value DECODE
     * path, which supports composite payloads (see [TypedSchemaValue]).
     */
    fun readPersistedDurableFunctionInvocation(): PersistedDurableFunctionInvocation {
        val ret = alloc(88, 8)
        hostReadPersistedDurableFunctionInvocation(ret)
        val timestampSeconds = loadLong(ret) // datetime.seconds @ 0 (u64)
        val timestampNanos = loadInt(ret + 8) // datetime.nanoseconds @ 8 (u32)
        val functionName = liftString(loadInt(ret + 16), loadInt(ret + 20)) // string @ 16
        val response = liftTypedSchemaValue(ret + 24) // typed-schema-value @ 24 (32B)
        val functionType = readDurableFunctionTypeInMemory(ret + 56) // durable-function-type @ 56 (24B)
        val entryVersion = when (loadByte(ret + 80).toInt() and 0xFF) { // enum @ 80
            0 -> OplogEntryVersion.V1
            else -> OplogEntryVersion.V2
        }
        return PersistedDurableFunctionInvocation(
            timestampSeconds,
            timestampNanos,
            functionName,
            response,
            functionType,
            entryVersion,
        )
    }
}
