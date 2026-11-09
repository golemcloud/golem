@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitSubtask
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeByte

// Raw bindings to the promise functions of golem:api/host@1.5.0 and create-webhook of golem:agent/host@2.0.0.
// promise-id = { agent-id: { component-id: uuid@0 (16 B), agent-id: string@16 }, oplog-idx: u64@24 } (32 B, align 8), passed flattened as
// (i64 high, i64 low, i32 ptr, i32 len, i64 oplog-idx). `get-promise-result.get` is an async function: imported async-lowered and waited out in place.
private const val API = "golem:api/host@1.5.0"
private const val AGENT = "golem:agent/host@2.0.0"

@kotlin.wasm.WasmImport(API, "create-promise")
private external fun hostCreatePromise(retPtr: Int)

@kotlin.wasm.WasmImport(API, "get-promise")
private external fun hostGetPromise(high: Long, low: Long, idPtr: Int, idLen: Int, oplogIdx: Long): Int

@kotlin.wasm.WasmImport(API, "complete-promise")
private external fun hostCompletePromise(high: Long, low: Long, idPtr: Int, idLen: Int, oplogIdx: Long, dataPtr: Int, dataLen: Int): Int

@kotlin.wasm.WasmImport(API, "[async-lower][method]get-promise-result.get")
private external fun hostPromiseResultGet(self: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(API, "[resource-drop]get-promise-result")
private external fun hostPromiseResultDrop(handle: Int)

@kotlin.wasm.WasmImport(AGENT, "create-webhook")
private external fun hostCreateWebhook(high: Long, low: Long, idPtr: Int, idLen: Int, oplogIdx: Long, retPtr: Int)

/**
 * A promise: a one-shot value some other party completes with bytes. Hand the [PromiseId] to whoever should complete it (another agent, an
 * external system through [Webhooks]) and [Promises.await] it. [oplogIdx] is an unsigned 64-bit index.
 */
data class PromiseId(val agentId: AgentId, val oplogIdx: Long)

/** Thrown by [Webhooks.create] when the host refuses to create the webhook. */
class WebhookException(val permissionDenied: Boolean, message: String) : RuntimeException(message)

private fun liftPromiseId(base: Int) = PromiseId(
    AgentId(liftComponentId(base), liftString(loadInt(base + 16), loadInt(base + 20))),
    loadLong(base + 24),
)

private inline fun <R> withPromiseId(id: PromiseId, call: (Long, Long, Int, Int, Long) -> R): R {
    val (ptr, len) = lowerStringToPtrLen(id.agentId.agentId)
    val uuid = id.agentId.componentId.uuid
    return call(uuid.highBits, uuid.lowBits, ptr, len, id.oplogIdx)
}

/** Durable promises (`golem:api/host`): create, await and complete. */
object Promises {
    /** Creates a promise owned by the calling agent. */
    fun create(): PromiseId {
        val ret = alloc(32, 8)
        hostCreatePromise(ret)
        return liftPromiseId(ret)
    }

    /**
     * Waits for the promise to be completed and returns its payload. The agent is suspended by the host while it waits and the wait survives a
     * restart. Only the agent that created the promise can await it.
     */
    fun await(id: PromiseId): ByteArray {
        val handle = withPromiseId(id, ::hostGetPromise)
        try {
            val ret = alloc(8, 4) // list<u8>: ptr@0, len@4
            awaitSubtask(hostPromiseResultGet(handle, ret))
            val ptr = loadInt(ret)
            return ByteArray(loadInt(ret + 4)) { loadByte(ptr + it) }
        } finally {
            hostPromiseResultDrop(handle)
        }
    }

    /** Completes the promise with [data]. Returns `false` if it was already completed. May be called from any agent. */
    fun complete(id: PromiseId, data: ByteArray): Boolean {
        val dataPtr = if (data.isEmpty()) 0 else alloc(data.size, 1).also { p -> for (i in data.indices) storeByte(p + i, data[i]) }
        return withPromiseId(id) { high, low, ptr, len, idx -> hostCompletePromise(high, low, ptr, len, idx, dataPtr, data.size) } != 0
    }
}

/** Webhooks (`golem:agent/host`): an HTTP URL whose POST body completes a promise. */
object Webhooks {
    /**
     * Creates a webhook for [promise] (created by this agent type) and returns its URL. A POST to the URL completes the promise with the request
     * body. The agent type must currently be deployed through an HTTP API.
     * @throws WebhookException if the host refuses.
     */
    fun create(promise: PromiseId): String {
        val ret = alloc(16, 4) // result<string, webhook-error>: tag@0; ok string@4; err: variant tag@4, string@8
        withPromiseId(promise) { high, low, ptr, len, idx -> hostCreateWebhook(high, low, ptr, len, idx, ret) }
        if (loadByte(ret).toInt() and 0xFF == 0) return liftString(loadInt(ret + 4), loadInt(ret + 8))
        val denied = loadByte(ret + 4).toInt() and 0xFF == 0
        throw WebhookException(denied, if (denied) "webhook creation denied" else liftString(loadInt(ret + 8), loadInt(ret + 12)))
    }
}
