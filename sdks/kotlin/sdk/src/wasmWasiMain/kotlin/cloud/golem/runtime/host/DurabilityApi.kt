@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.host

import cloud.golem.runtime.TypedSchemaValue
import cloud.golem.runtime.liftTypedSchemaValue
import cloud.golem.runtime.lowerTypedSchemaValueInto
import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.writeStringField

// Raw canonical-ABI import bindings to golem:durability/durability@1.6.0.
//
// Golem 1.6.0 replaced the begin/end/persist/read durable-function primitives (and the atomic
// regions, persistence levels and oplog-index rewinding built on them) with *custom durable
// invocations*: begin-custom-durable-invocation returns either a `live` handle -- the guest runs
// the side effect and then finish()es it with the response -- or the `replayed` persisted result.
//
// The async variants (begin-custom-durable-invocation-async, finish-async) and
// exchange-custom-invocation-context exist for guests that interleave many invocations on one
// thread (the Rust SDK scopes its futures with them). This SDK is synchronous, so a custom
// invocation is fully nested inside the caller's stack and none of them are needed; `enter` is
// likewise only used by the async runners.
//
// Core signatures, derived from the canonical ABI (wasm-tools validates them at component-new time):
//  - typed-schema-value is a 32-byte record of eight i32 words (graph{type-nodes ptr/len, defs
//    ptr/len, root} + value{nodes ptr/len, root}); it is passed *flattened*, because the whole
//    parameter list stays within the 16-word MAX_FLAT_PARAMS budget.
//  - durable-function-type is a 4-case payload-free variant, i.e. a single i32.
//  - the `custom-durable-invocation` result is returned through a guest-allocated return pointer.

private const val TSV_WORDS = 8

// begin-custom-durable-invocation(function-name: string, request: typed-schema-value,
//   function-type: durable-function-type) -> custom-durable-invocation  (11 flat params + retptr)
@kotlin.wasm.WasmImport("golem:durability/durability@1.6.0", "observe-function-call")
private external fun hostObserveFunctionCall(ifacePtr: Int, ifaceLen: Int, funcPtr: Int, funcLen: Int)

@kotlin.wasm.WasmImport("golem:durability/durability@1.6.0", "begin-custom-durable-invocation")
private external fun hostBeginCustomDurableInvocation(
    namePtr: Int,
    nameLen: Int,
    r0: Int,
    r1: Int,
    r2: Int,
    r3: Int,
    r4: Int,
    r5: Int,
    r6: Int,
    r7: Int,
    functionType: Int,
    retPtr: Int,
)

// [static]live-custom-durable-invocation.finish(this: own<live-custom-durable-invocation>,
//   response: typed-schema-value, forced-commit: bool)
@kotlin.wasm.WasmImport("golem:durability/durability@1.6.0", "[static]live-custom-durable-invocation.finish")
private external fun hostFinish(
    handle: Int,
    r0: Int,
    r1: Int,
    r2: Int,
    r3: Int,
    r4: Int,
    r5: Int,
    r6: Int,
    r7: Int,
    forcedCommit: Int,
)

@kotlin.wasm.WasmImport("golem:durability/durability@1.6.0", "[resource-drop]live-custom-durable-invocation")
private external fun hostLiveDrop(handle: Int)

/**
 * `golem:api/oplog@1.5.0`'s `wrapped-function-type` (aliased `durable-function-type`): how a custom
 * durable side effect behaves on replay and what it touches. Declaration order is the ABI order.
 */
enum class DurableFunctionType {
    /** Reads the agent's local state (for example the local file system or a random generator). */
    ReadLocal,

    /** Writes the agent's local state (for example the local file system). */
    WriteLocal,

    /** Reads external state (for example a key-value store). */
    ReadRemote,

    /** Manipulates external state (for example an RPC call). */
    WriteRemote,
}

/** Reads a (1-byte) `durable-function-type` at [base]. */
internal fun readDurableFunctionType(base: Int): DurableFunctionType {
    val tag = loadByte(base).toInt() and 0xFF
    return DurableFunctionType.entries.getOrNull(tag) ?: error("native readDurableFunctionType: unknown tag $tag")
}

/** WIT `oplog-entry-version` enum (golem:durability@1.6.0). */
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

/**
 * A live custom durable invocation: the side effect has not been recorded yet. Exactly one of
 * [finish] (record the response) or [drop] (abandon it, e.g. the body threw) must be called.
 */
class LiveCustomDurableInvocation internal constructor(private var handle: Int) {
    private var active = true

    /** Records [response] as the result of this invocation, optionally forcing an oplog commit. */
    fun finish(response: TypedSchemaValue, forcedCommit: Boolean = false) {
        check(active) { "custom durable invocation is no longer active" }
        active = false
        val tsv = alloc(TSV_WORDS * 4, 4)
        lowerTypedSchemaValueInto(tsv, response)
        hostFinish(
            handle,
            loadInt(tsv), loadInt(tsv + 4), loadInt(tsv + 8), loadInt(tsv + 12),
            loadInt(tsv + 16), loadInt(tsv + 20), loadInt(tsv + 24), loadInt(tsv + 28),
            if (forcedCommit) 1 else 0,
        )
    }

    /** Abandons this invocation without recording a response (releases the host resource). */
    fun drop() {
        if (active) {
            active = false
            hostLiveDrop(handle)
        }
    }
}

/** What [DurabilityApi.beginCustomDurableInvocation] decided for this call. */
sealed class CustomDurableInvocation {
    /** First execution: run the side effect, then [LiveCustomDurableInvocation.finish] it. */
    class Live(val invocation: LiveCustomDurableInvocation) : CustomDurableInvocation()

    /** Replay: the side effect already ran; use the persisted response instead of running it again. */
    class Replayed(val invocation: PersistedDurableFunctionInvocation) : CustomDurableInvocation()
}

/**
 * Native SDK access to `golem:durability/durability@1.6.0` (custom durable invocations),
 * mirroring the Scala SDK's `DurabilityApi` (`sdks/scala/core/js/.../host/DurabilityApi.scala`).
 *
 * Request and response are self-describing [TypedSchemaValue]s (any composite shape).
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

    /**
     * Begins a custom durable invocation named [functionName]. Prefer [durable], which also does the
     * replay validation and the finish/drop bookkeeping.
     */
    fun beginCustomDurableInvocation(
        functionName: String,
        request: TypedSchemaValue,
        functionType: DurableFunctionType,
    ): CustomDurableInvocation {
        val name = alloc(8, 4)
        writeStringField(name, 0, functionName)
        val tsv = alloc(TSV_WORDS * 4, 4)
        lowerTypedSchemaValueInto(tsv, request)
        // custom-durable-invocation: tag@0, payload@8. live(own<resource>): handle@8. replayed(persisted-
        // durable-function-invocation: timestamp@0(16) function-name@16(8) response@24(32) function-type@56(1)
        // entry-version@57(1), size 64 align 8): record @8..72.
        val ret = alloc(72, 8)
        hostBeginCustomDurableInvocation(
            loadInt(name), loadInt(name + 4),
            loadInt(tsv), loadInt(tsv + 4), loadInt(tsv + 8), loadInt(tsv + 12),
            loadInt(tsv + 16), loadInt(tsv + 20), loadInt(tsv + 24), loadInt(tsv + 28),
            functionType.ordinal,
            ret,
        )
        return when (val tag = loadByte(ret).toInt() and 0xFF) {
            0 -> CustomDurableInvocation.Live(LiveCustomDurableInvocation(loadInt(ret + 8)))
            1 -> CustomDurableInvocation.Replayed(liftPersisted(ret + 8))
            else -> error("native beginCustomDurableInvocation: unknown custom-durable-invocation tag $tag")
        }
    }

    private fun liftPersisted(base: Int): PersistedDurableFunctionInvocation = PersistedDurableFunctionInvocation(
        timestampSeconds = loadLong(base), // datetime.seconds @ 0
        timestampNanoseconds = loadInt(base + 8), // datetime.nanoseconds @ 8
        functionName = liftString(loadInt(base + 16), loadInt(base + 20)),
        response = liftTypedSchemaValue(base + 24),
        functionType = readDurableFunctionType(base + 56),
        entryVersion = if ((loadByte(base + 57).toInt() and 0xFF) == 0) OplogEntryVersion.V1 else OplogEntryVersion.V2,
    )

    /**
     * Runs [body] as a durable side effect. On the first execution [body] runs and its result is
     * recorded; on replay the recorded result is returned and [body] does NOT run again. If [body]
     * throws, the invocation is abandoned (nothing recorded) and the exception propagates.
     *
     * [iface] and [function] name the operation (`"iface::function"`, or just [function] when
     * [iface] is empty). [request] is recorded with the call; [forcedCommit] commits the oplog
     * immediately after recording the response.
     */
    fun durable(
        iface: String,
        function: String,
        functionType: DurableFunctionType,
        request: TypedSchemaValue,
        forcedCommit: Boolean = false,
        body: () -> TypedSchemaValue,
    ): TypedSchemaValue {
        observeFunctionCall(iface, function)
        val functionName = if (iface.isEmpty()) function else "$iface::$function"
        return when (val invocation = beginCustomDurableInvocation(functionName, request, functionType)) {
            is CustomDurableInvocation.Live -> {
                val live = invocation.invocation
                val response = try {
                    body()
                } catch (t: Throwable) {
                    live.drop()
                    throw t
                }
                live.finish(response, forcedCommit)
                response
            }
            is CustomDurableInvocation.Replayed -> {
                val persisted = invocation.invocation
                check(persisted.functionName == functionName) {
                    "durable replay mismatch: expected function '$functionName', oplog has '${persisted.functionName}'"
                }
                check(persisted.functionType == functionType) {
                    "durable replay mismatch for '$functionName': expected function type $functionType, oplog has ${persisted.functionType}"
                }
                persisted.response
            }
        }
    }
}
