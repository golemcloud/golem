@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

import cloud.golem.wasm.StreamStatus
import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitOperation
import cloud.golem.wasm.awaitSubtask
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.withArenaMark

// Provider-side streamed I/O of a tool body (golem:tool/guest `invoke`): stdin arrives as a `stream<byte-stream-item>`
// and stdout/stderr as owned `tool-output-writer` resources (golem:tool/streams). The body is plain synchronous code,
// so each async call is imported async-lowered and waited out in place (`awaitSubtask` / `awaitOperation`).
private const val STREAMS = "golem:tool/streams@0.1.0"
private const val GUEST_EXPORT = "[export]golem:tool/guest@0.1.0"

@kotlin.wasm.WasmImport(STREAMS, "[async-lower][method]tool-output-writer.write")
private external fun hostWriterWrite(self: Int, ptr: Int, len: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(STREAMS, "[async-lower][method]tool-output-writer.finish")
private external fun hostWriterFinish(self: Int, retPtr: Int): Int

// fail(reason: byte-stream-failure): self + the flattened variant (tag, string ptr, string len) + retptr exceed the
// async-lower limit of 4, so the arguments go through one in-memory record { self@0, reason@4 (12 B) }.
@kotlin.wasm.WasmImport(STREAMS, "[async-lower][method]tool-output-writer.fail")
private external fun hostWriterFail(args: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(STREAMS, "[resource-drop]tool-output-writer")
private external fun hostWriterDrop(handle: Int)

@kotlin.wasm.WasmImport(GUEST_EXPORT, "[async-lower][stream-read-0]invoke")
private external fun hostStdinRead(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport(GUEST_EXPORT, "[stream-drop-readable-0]invoke")
private external fun hostStdinDropReadable(stream: Int)

private const val WRITE_CHUNK = 64 * 1024
private const val ITEM_SIZE = 16 // byte-stream-item = result<list<u8>, byte-stream-failure>

/** Thrown by [ToolInput.read] when the caller ended stdin with a failure instead of a clean end of input. */
class ToolStreamException(val failure: ToolStreamFailure) : RuntimeException("tool stdin failed: $failure")

/**
 * The tool body's view of its attached streams. A stream is `null` when the caller did not attach it (declare
 * `stdin` / `stdout` / `stderr` on `@Tool`, and treat them as optional).
 */
class ToolStreams internal constructor(
    /** The caller's stdin, or `null` if none was supplied. */
    val stdin: ToolInput?,
    /** The stdout writer, or `null` if the caller did not attach one. */
    val stdout: ToolOutput?,
    /** The stderr writer, or `null` if the caller did not attach one. */
    val stderr: ToolOutput?,
) {
    /** Called by the runtime after the body returns (`completed`) or throws: closes every stream endpoint. */
    internal fun release(completed: Boolean) {
        // Returning normally finishes any writer the body left open; an exception drops it, which the host reads as
        // `abandoned` (the first terminal wins, so a writer the body already finished or failed is unaffected).
        if (completed) {
            stdout?.finishQuietly()
            stderr?.finishQuietly()
        }
        stdout?.dropHandle()
        stderr?.dropHandle()
        stdin?.dropHandle()
    }

    internal companion object {
        /** An empty set, for descriptors without stream specs. */
        val NONE = ToolStreams(null, null, null)
    }
}

/** Reads the caller's stdin. */
class ToolInput internal constructor(private val stream: Int) : ToolStdin {
    private val item = alloc(ITEM_SIZE, 4)
    private var ended = false
    private var moved = false

    /** Gives the stream handle away (forwarding stdin to a wrapped tool): the caller owns it now. */
    internal fun takeStreamHandle(): Int {
        check(!moved) { "this stdin was already forwarded" }
        moved = true
        ended = true
        return stream
    }

    /** True if the stream handle was handed on and must not be dropped here. */
    internal val isForwarded: Boolean get() = moved

    /**
     * The next chunk of stdin (never empty), or `null` at the clean end of input.
     * @throws ToolStreamException if the caller ended stdin with a failure.
     */
    fun read(): ByteArray? {
        if (ended) return null
        val status = awaitOperation(stream, hostStdinRead(stream, item, 1))
        if (StreamStatus.count(status) == 0) {
            ended = true
            return null
        }
        if (StreamStatus.code(status) != StreamStatus.COMPLETED) ended = true
        if (loadByte(item).toInt() and 0xFF == 0) {
            val ptr = loadInt(item + 4)
            val len = loadInt(item + 8)
            return ByteArray(len) { loadByte(ptr + it) }
        }
        ended = true
        throw ToolStreamException(
            when (loadByte(item + 4).toInt() and 0xFF) {
                0 -> ToolStreamFailure.Cancelled
                1 -> ToolStreamFailure.Abandoned
                2 -> ToolStreamFailure.ResourceExhausted
                else -> ToolStreamFailure.Failed(liftString(loadInt(item + 8), loadInt(item + 12)))
            },
        )
    }

    /**
     * Reads stdin to its end.
     * @throws ToolStreamException with [ToolStreamFailure.ResourceExhausted] if it holds more than [maxBytes].
     */
    fun readAll(maxBytes: Int = CAPTURED_BYTES_MAX): ByteArray {
        Cap.requireLimit(maxBytes, CAPTURED_BYTES_MAX, "maxBytes")
        val tally = Tally(maxBytes)
        val chunks = ArrayList<ByteArray>()
        while (true) {
            val chunk = read() ?: break
            if (!tally.add(chunk.size)) throw ToolStreamException(ToolStreamFailure.ResourceExhausted)
            chunks.add(chunk)
        }
        val out = ByteArray(chunks.sumOf { it.size })
        var at = 0
        for (c in chunks) {
            c.copyInto(out, at)
            at += c.size
        }
        return out
    }

    internal fun dropHandle() {
        if (!moved) hostStdinDropReadable(stream)
    }
}

/** Writes to the tool's stdout or stderr. The first terminal ([finish] or [fail]) is final. */
class ToolOutput internal constructor(private val writer: Int) : ByteSink {
    private val result = alloc(32, 4) // result<_, stream-write-error>: tag@0 (0 = ok)
    private var terminal = false

    /**
     * Writes [bytes] (in slices; an empty array writes nothing). Returns `false` once the caller has stopped reading or
     * the stream has ended, in which case the rest of the output is dropped.
     */
    override fun write(bytes: ByteArray): Boolean {
        if (terminal) return false
        if (bytes.isEmpty()) return true
        // Each slice is awaited before the next, so one scratch buffer serves the call and is released afterwards.
        return withArenaMark {
            val slice = alloc(minOf(WRITE_CHUNK, bytes.size), 1)
            var offset = 0
            var accepted = true
            while (accepted && offset < bytes.size) {
                val len = minOf(WRITE_CHUNK, bytes.size - offset)
                for (i in 0 until len) storeByte(slice + i, bytes[offset + i])
                awaitSubtask(hostWriterWrite(writer, slice, len, result))
                if (loadByte(result).toInt() and 0xFF != 0) {
                    terminal = true
                    accepted = false
                } else {
                    offset += len
                }
            }
            accepted
        }
    }

    /** Ends the stream cleanly. Returns `false` if it had already ended. */
    fun finish(): Boolean {
        if (terminal) return false
        terminal = true
        awaitSubtask(hostWriterFinish(writer, result))
        return loadByte(result).toInt() and 0xFF == 0
    }

    /** Ends the stream with a recoverable failure the caller sees as the stream's final item. */
    fun fail(failure: ToolStreamFailure): Boolean {
        if (terminal) return false
        terminal = true
        val args = alloc(16, 4)
        storeInt(args, writer)
        val reason = args + 4
        storeInt(reason + 4, 0)
        storeInt(reason + 8, 0)
        when (failure) {
            is ToolStreamFailure.Cancelled -> storeByte(reason, 0)
            is ToolStreamFailure.Abandoned -> storeByte(reason, 1)
            is ToolStreamFailure.ResourceExhausted -> storeByte(reason, 2)
            is ToolStreamFailure.Failed -> {
                storeByte(reason, 3)
                val bytes = failure.message.encodeToByteArray()
                val ptr = alloc(maxOf(bytes.size, 1), 1)
                for (i in bytes.indices) storeByte(ptr + i, bytes[i])
                storeInt(reason + 4, ptr)
                storeInt(reason + 8, bytes.size)
            }
        }
        awaitSubtask(hostWriterFail(args, result))
        return loadByte(result).toInt() and 0xFF == 0
    }

    internal fun finishQuietly() {
        if (!terminal) finish()
    }

    internal fun dropHandle() = hostWriterDrop(writer)
}
