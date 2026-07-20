@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftParamRecord
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.resetHeap
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt

// snapshot record { payload: list<u8> (ptr@0,len@4), mime-type: string (ptr@8,len@12) }, size 16.
// result<_, string> lowered { tag@0 (0=ok,1=err), errPtr@4, errLen@8 }, size 12.
private const val SNAP_SIZE = 16
private const val SNAP_ALIGN = 4
private const val SNAP_PAYLOAD_PTR = 0
private const val SNAP_PAYLOAD_LEN = 4
private const val SNAP_MIME_PTR = 8
private const val SNAP_MIME_LEN = 12
private const val RES_SIZE = 12
private const val RES_ALIGN = 4
private const val MIME = "application/octet-stream"

private fun writeBytesField(recordBase: Int, ptrOff: Int, lenOff: Int, bytes: ByteArray) {
    val p = if (bytes.isEmpty()) 0 else alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(p + i, bytes[i])
    storeInt(recordBase + ptrOff, p)
    storeInt(recordBase + lenOff, bytes.size)
}

private fun readBytesField(base: Int, ptrOff: Int, lenOff: Int): ByteArray {
    val p = loadInt(base + ptrOff)
    val n = loadInt(base + lenOff)
    return ByteArray(n) { loadByte(p + it) }
}

/**
 * `save: func() -> snapshot`. Empty payload if the live agent didn't opt in (no snapshotCodec);
 * otherwise the principal-carrying envelope around the auto-serialized state. Never throws.
 */
fun saveSnapshot(): Int {
    val codec = NativeAgentRuntime.currentDescriptor?.snapshotCodec
    val instance = NativeAgentRuntime.current
    val payload = if (codec == null || instance == null) {
        ByteArray(0)
    } else {
        val state = codec.save(instance)
        SnapshotEnvelope.encode(PrincipalBytes.encode(NativeAgentRuntime.initializationPrincipal), state)
    }
    val rec = alloc(SNAP_SIZE, SNAP_ALIGN)
    writeBytesField(rec, SNAP_PAYLOAD_PTR, SNAP_PAYLOAD_LEN, payload)
    writeBytesField(rec, SNAP_MIME_PTR, SNAP_MIME_LEN, MIME.encodeToByteArray())
    return rec
}

// Reclaim the bump heap after the host has consumed the returned record. The canonical ABI runs
// cabi_post AFTER the caller reads the return value, so resetting here is safe -- and necessary:
// the periodic/"every(n)" snapshot cadences invoke save-snapshot repeatedly, and without this each
// call would permanently advance the bump pointer (mirrors cabiPostInvoke/cabiPostInitialize).
fun cabiPostSaveSnapshot(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}

/**
 * On a snapshot-based (manual) update, the host recovers the worker by calling `load-snapshot`
 * on a FRESH wasm instance WITHOUT a preceding `initialize` (worker-executor skips the original
 * initialize oplog entry -- it is inside the deleted region up to the snapshot index). So unlike
 * `invoke`, `load` cannot assume the agent already exists: it must reconstruct it. We rebuild it
 * exactly as `initialize` would -- construct via the descriptor factory from the constructor
 * parameters carried in the agent's own id (obtained via get-self-metadata + parse-agent-id) --
 * and only then restore the saved state onto it.
 */
private fun ensureAgentReconstructed() {
    if (NativeAgentRuntime.current != null) return
    val agentId = HostApi.getSelfMetadata().agentId.agentId
    val ref = HostApi.parseAgentIdConstructorParams(agentId)
        ?: error("load-snapshot: cannot parse self agent-id '$agentId' to reconstruct the agent")
    val descriptor = NativeAgentRuntime.lookup(ref.agentTypeName)
        ?: error("load-snapshot: unknown agent type '${ref.agentTypeName}' for agent-id '$agentId'")
    val params = liftParamRecord(ref.paramsValueTreePtr, descriptor.constructorParams.map { it.witType })
    NativeAgentRuntime.current = descriptor.factory(params)
    NativeAgentRuntime.currentDescriptor = descriptor
}

/**
 * `load: func(snapshot) -> result<_, string>`. Restores the initialization principal from the
 * envelope, reconstructs the agent instance if the host didn't `initialize` first (snapshot
 * recovery), then hands the state bytes to the descriptor's codec (load). Empty payload = no-op Ok.
 */
fun loadSnapshot(argsPtr: Int): Int {
    val res = alloc(RES_SIZE, RES_ALIGN)
    try {
        val payload = readBytesField(argsPtr, SNAP_PAYLOAD_PTR, SNAP_PAYLOAD_LEN)
        if (payload.isNotEmpty()) {
            val env = SnapshotEnvelope.decode(payload)
            val principal = PrincipalBytes.decode(env.principal)
            NativeAgentRuntime.initializationPrincipal = principal
            NativeAgentRuntime.currentPrincipal = principal
            ensureAgentReconstructed()
            val codec = NativeAgentRuntime.currentDescriptor?.snapshotCodec
            val instance = NativeAgentRuntime.current
            if (codec != null && instance != null) codec.load(instance, env.state)
        }
        storeByte(res, 0)
    } catch (e: Throwable) {
        storeByte(res, 1)
        val msg = (e.message ?: "snapshot load failed").encodeToByteArray()
        val p = alloc(msg.size, 1)
        for (i in msg.indices) storeByte(p + i, msg[i])
        storeInt(res + 4, p)
        storeInt(res + 8, msg.size)
    }
    return res
}

fun cabiPostLoadSnapshot(@Suppress("UNUSED_PARAMETER") resultPtr: Int) {
    resetHeap()
}

/**
 * Canonical-ABI flat adapter for `load: func(snapshot) -> result<_, string>`.
 *
 * The Wasm Canonical ABI flattens `record { payload: list<u8>, mime-type: string }` into four
 * I32 parameters (payloadPtr, payloadLen, mimeTypePtr, mimeTypeLen) rather than passing a
 * single pointer to an in-memory record.  The `@WasmExport` generated by KSP uses this
 * function signature so that wasm-tools component embed sees `[I32, I32, I32, I32] -> [I32]`,
 * which matches the WIT-derived type for the `golem:api/load-snapshot@1.5.0#load` export.
 */
fun loadSnapshotFlat(payloadPtr: Int, payloadLen: Int, mimeTypePtr: Int, mimeTypeLen: Int): Int {
    // Assemble the snapshot record in our linear-memory heap so the existing
    // loadSnapshot(argsPtr) helper (which reads fields at fixed offsets) can process it.
    val rec = alloc(SNAP_SIZE, SNAP_ALIGN)
    storeInt(rec + SNAP_PAYLOAD_PTR, payloadPtr)
    storeInt(rec + SNAP_PAYLOAD_LEN, payloadLen)
    storeInt(rec + SNAP_MIME_PTR, mimeTypePtr)
    storeInt(rec + SNAP_MIME_LEN, mimeTypeLen)
    return loadSnapshot(rec)
}
