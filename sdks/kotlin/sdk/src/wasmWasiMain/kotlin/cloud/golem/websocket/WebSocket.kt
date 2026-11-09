@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.websocket

import cloud.golem.runtime.lowerStringToPtrLen
import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitSubtask
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadShort
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.withArenaMark

// Raw canonical-ABI bindings to golem:websocket/client@1.5.0. `receive` and `receive-with-timeout` are async functions, imported
// async-lowered and waited out with `awaitSubtask`; connect / send / close are plain calls. Layouts come from `wasm-tools component wit
// --json`: message = variant { text(string), binary(list<u8>) } (12 B: tag@0, ptr@4, len@8); error (20 B: tag@0, string / list at @4,
// `closed(option<close-info { code: u16, reason: string }>)` = option tag@4, code@8, reason ptr@12 / len@16); every result is 24 B with
// the payload at @4.
private const val WS = "golem:websocket/client@1.5.0"
private const val CONNECTION = "websocket-connection"

// connect(url, headers: option<list<tuple<string, string>>>, reconstruction-policy: option<enum>) -> result<websocket-connection, error>
@kotlin.wasm.WasmImport(WS, "[static]$CONNECTION.connect")
private external fun hostConnect(
    urlPtr: Int,
    urlLen: Int,
    headersTag: Int,
    headersPtr: Int,
    headersLen: Int,
    policyTag: Int,
    policy: Int,
    retPtr: Int,
)

@kotlin.wasm.WasmImport(WS, "[method]$CONNECTION.send")
private external fun hostSend(self: Int, messageTag: Int, dataPtr: Int, dataLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport(WS, "[async-lower][method]$CONNECTION.receive")
private external fun hostReceive(self: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(WS, "[async-lower][method]$CONNECTION.receive-with-timeout")
private external fun hostReceiveWithTimeout(self: Int, timeoutMs: Long, retPtr: Int): Int

@kotlin.wasm.WasmImport(WS, "[method]$CONNECTION.close")
private external fun hostClose(self: Int, codeTag: Int, code: Int, reasonTag: Int, reasonPtr: Int, reasonLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport(WS, "[resource-drop]$CONNECTION")
private external fun hostDrop(handle: Int)

/** What the runtime does when the connection is lost (for example across a restart of the agent). */
enum class ReconstructionPolicy {
    /** Reconnect on the agent's behalf (the default). */
    RECONNECT_AUTOMATICALLY,

    /** Report the loss to the agent as [WebSocketError.SessionLost]. */
    REPORT_CONNECTION_LOSS,
}

/** A WebSocket message. */
sealed class WebSocketMessage {
    data class Text(val text: String) : WebSocketMessage()

    class Binary(val bytes: ByteArray) : WebSocketMessage() {
        override fun equals(other: Any?): Boolean = other is Binary && other.bytes.contentEquals(bytes)

        override fun hashCode(): Int = bytes.contentHashCode()

        override fun toString(): String = "Binary(${bytes.size} bytes)"
    }
}

/** `golem:websocket/client`'s `error`. */
sealed class WebSocketError {
    data class ConnectionFailure(val message: String) : WebSocketError()

    data class SendFailure(val message: String) : WebSocketError()

    data class ReceiveFailure(val message: String) : WebSocketError()

    data class ProtocolError(val message: String) : WebSocketError()

    /** The peer closed the connection; [code] and [reason] are present when it sent a close frame. */
    data class Closed(val code: Int?, val reason: String?) : WebSocketError()

    data class Other(val message: String) : WebSocketError()

    /** The connection was lost and the reconstruction policy is [ReconstructionPolicy.REPORT_CONNECTION_LOSS]. */
    object SessionLost : WebSocketError()
}

/** Thrown when a WebSocket call fails. */
class WebSocketException(val error: WebSocketError) : RuntimeException("websocket: $error")

private fun liftError(base: Int): WebSocketError {
    fun text() = liftString(loadInt(base + 4), loadInt(base + 8))
    return when (val tag = loadByte(base).toInt() and 0xFF) {
        0 -> WebSocketError.ConnectionFailure(text())
        1 -> WebSocketError.SendFailure(text())
        2 -> WebSocketError.ReceiveFailure(text())
        3 -> WebSocketError.ProtocolError(text())
        4 -> if (loadByte(base + 4).toInt() and 0xFF == 0) {
            WebSocketError.Closed(null, null)
        } else {
            WebSocketError.Closed(loadShort(base + 8).toInt() and 0xFFFF, liftString(loadInt(base + 12), loadInt(base + 16)))
        }
        5 -> WebSocketError.Other(text())
        6 -> WebSocketError.SessionLost
        else -> error("unknown websocket error case $tag")
    }
}

private fun liftMessage(base: Int): WebSocketMessage {
    val ptr = loadInt(base + 4)
    val len = loadInt(base + 8)
    return if (loadByte(base).toInt() and 0xFF == 0) {
        WebSocketMessage.Text(liftString(ptr, len))
    } else {
        WebSocketMessage.Binary(ByteArray(len) { loadByte(ptr + it) })
    }
}

/** `result<_, error>` / `result<T, error>`: tag@0, payload and error at @4. Throws on error. */
private fun checkOk(ret: Int) {
    if (loadByte(ret).toInt() and 0xFF != 0) throw WebSocketException(liftError(ret + 4))
}

/**
 * A WebSocket client connection, driven by the Golem runtime: every call is recorded durably, so a restarted agent sees the same messages
 * and either reconnects (the default) or gets [WebSocketError.SessionLost] with [ReconstructionPolicy.REPORT_CONNECTION_LOSS]. All
 * calls block. [close] when done (the handle is not tied to Kotlin/Wasm GC).
 */
class WebSocketConnection private constructor(private var handle: Int) {
    private fun live(): Int {
        check(handle != 0) { "this websocket connection was closed" }
        return handle
    }

    /** Sends [message]. @throws WebSocketException on failure. */
    fun send(message: WebSocketMessage) {
        val (tag, ptr, len) = when (message) {
            is WebSocketMessage.Text -> message.text.encodeToByteArray().let { Triple(0, it, it.size) }
            is WebSocketMessage.Binary -> Triple(1, message.bytes, message.bytes.size)
        }
        val self = live()
        withArenaMark {
            val data = alloc(maxOf(len, 1), 1)
            for (i in 0 until len) storeByte(data + i, ptr[i])
            val ret = alloc(24, 4)
            hostSend(self, tag, data, len, ret)
            checkOk(ret)
        }
    }

    /** Sends a text message. */
    fun send(text: String) = send(WebSocketMessage.Text(text))

    /** Waits for the next message. @throws WebSocketException when the connection fails or the peer closed it. */
    fun receive(): WebSocketMessage {
        val self = live()
        return withArenaMark {
            val ret = alloc(24, 4)
            awaitSubtask(hostReceive(self, ret))
            checkOk(ret)
            liftMessage(ret + 4)
        }
    }

    /** Waits up to [timeoutMs] for the next message; null if none arrived in time. */
    fun receive(timeoutMs: Long): WebSocketMessage? {
        val self = live()
        return withArenaMark {
            val ret = alloc(24, 4) // result<option<message>, error>: tag@0, option tag@4, message@8
            awaitSubtask(hostReceiveWithTimeout(self, timeoutMs, ret))
            checkOk(ret)
            if (loadByte(ret + 4).toInt() and 0xFF == 0) null else liftMessage(ret + 8)
        }
    }

    /** Closes the connection (optionally with a close [code] and [reason]) and releases the handle. */
    fun close(code: Int? = null, reason: String? = null) {
        val self = handle
        if (self == 0) return
        handle = 0
        try {
            withArenaMark {
                val ret = alloc(24, 4)
                val (rp, rl) = if (reason == null) 0 to 0 else lowerStringToPtrLen(reason)
                hostClose(self, if (code == null) 0 else 1, code ?: 0, if (reason == null) 0 else 1, rp, rl, ret)
                checkOk(ret)
            }
        } finally {
            hostDrop(self)
        }
    }

    /** Releases the handle without sending a close frame. Idempotent. */
    fun release() {
        if (handle != 0) {
            hostDrop(handle)
            handle = 0
        }
    }

    companion object {
        /**
         * Connects to [url] (`ws://` or `wss://`) with optional request [headers].
         * @throws WebSocketException if the connection fails.
         */
        fun connect(
            url: String,
            headers: List<Pair<String, String>>? = null,
            reconstructionPolicy: ReconstructionPolicy? = null,
        ): WebSocketConnection {
            val (up, ul) = lowerStringToPtrLen(url)
            val list = if (headers.isNullOrEmpty()) 0 else alloc(headers.size * 16, 4)
            headers?.forEachIndexed { i, (name, value) ->
                val (np, nl) = lowerStringToPtrLen(name)
                val (vp, vl) = lowerStringToPtrLen(value)
                storeInt(list + i * 16, np)
                storeInt(list + i * 16 + 4, nl)
                storeInt(list + i * 16 + 8, vp)
                storeInt(list + i * 16 + 12, vl)
            }
            val ret = alloc(24, 4)
            hostConnect(
                up,
                ul,
                if (headers == null) 0 else 1,
                list,
                headers?.size ?: 0,
                if (reconstructionPolicy == null) 0 else 1,
                reconstructionPolicy?.ordinal ?: 0,
                ret,
            )
            checkOk(ret)
            return WebSocketConnection(loadInt(ret + 4))
        }
    }
}
