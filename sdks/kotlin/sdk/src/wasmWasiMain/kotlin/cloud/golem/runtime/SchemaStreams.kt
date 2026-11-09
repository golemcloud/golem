@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

import cloud.golem.runtime.wasi.Logging
import cloud.golem.wasm.StreamStatus
import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitOperation
import cloud.golem.wasm.awaitSubtask
import cloud.golem.wasm.buildSchemaValueTree
import cloud.golem.wasm.liftSingleValue
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.resetHeap
import cloud.golem.wasm.withArenaMark

// `golem:core/types@2.0.0`'s `schema-value-stream`: an affine wrapper around a native `stream<schema-value-tree>` that can sit anywhere in a
// schema value. `wrap(reader)` turns a native stream's readable end into a handle (outgoing), `unwrap(handle)` gives the native reader back
// (incoming). Both are async statics, imported async-lowered and waited out with `awaitSubtask`; the stream element operations are the
// async-lowered `[stream-*-0]` forms of those two functions (index 0 = the stream in the signature). An element is one `schema-value-tree`
// (12 B: value-nodes ptr, len, root). Verified on a real 1.6.0 executor (Kotlin producer and consumer over RPC).
private const val TYPES = "golem:core/types@2.0.0"
private const val WRAP = "[static]schema-value-stream.wrap"
private const val UNWRAP = "[static]schema-value-stream.unwrap"

@kotlin.wasm.WasmImport(TYPES, "[async-lower]$WRAP")
private external fun hostWrap(readable: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(TYPES, "[stream-new-0]$WRAP")
private external fun hostWrapStreamNew(): Long

@kotlin.wasm.WasmImport(TYPES, "[async-lower][stream-write-0]$WRAP")
private external fun hostWrapStreamWrite(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport(TYPES, "[stream-drop-writable-0]$WRAP")
private external fun hostWrapStreamDropWritable(stream: Int)

@kotlin.wasm.WasmImport(TYPES, "[async-lower]$UNWRAP")
private external fun hostUnwrap(handle: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(TYPES, "[async-lower][stream-read-0]$UNWRAP")
private external fun hostUnwrapStreamRead(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport(TYPES, "[stream-drop-readable-0]$UNWRAP")
private external fun hostUnwrapStreamDropReadable(stream: Int)

/**
 * The writing end of an outgoing `stream<T>`: [write] one element at a time (blocking until the reader takes it). The host calls live in
 * closures created by [SchemaStreams.createOutgoing], so merely referencing this type does not link the stream imports.
 */
class SchemaStreamSink internal constructor(private val writeElement: (SchemaValue) -> Boolean, private val finishStream: () -> Unit) {
    /** Writes [value]. Returns false once the reader has gone away (the rest of the output is dropped). */
    fun write(value: SchemaValue): Boolean = writeElement(value)

    internal fun finish() = finishStream()
}

/** The reading end of an incoming `stream<T>`. */
class SchemaStreamReader internal constructor(private var handle: Int, private val elementWitType: String) {
    private var stream = 0
    private var done = false

    /** The next element, or null at the end of the stream. */
    fun next(): SchemaValue? {
        if (done) return null
        if (stream == 0) {
            val ret = alloc(4, 4)
            awaitSubtask(hostUnwrap(handle, ret))
            stream = loadInt(ret)
            handle = 0 // consumed by unwrap
        }
        return withArenaMark {
            val buffer = alloc(12, 4)
            val status = awaitOperation(stream, hostUnwrapStreamRead(stream, buffer, 1))
            if (StreamStatus.count(status) == 0) {
                close()
                null
            } else {
                if (StreamStatus.code(status) != StreamStatus.COMPLETED) done = true
                try {
                    liftSingleValue(buffer, elementWitType)
                } catch (e: Throwable) {
                    close()
                    throw e
                }
            }
        }
    }

    /** True if the stream was never read, so its handle can still be handed on (forwarded). */
    internal val isUntouched: Boolean get() = stream == 0 && !done && handle != 0

    /** Gives the unread handle away (the caller now owns it). */
    fun takeHandleForForwarding(): Int {
        check(isUntouched) { "this stream was already read or moved" }
        val h = handle
        handle = 0
        done = true
        return h
    }

    /** Releases the stream. Idempotent. */
    fun close() {
        done = true
        if (stream != 0) {
            hostUnwrapStreamDropReadable(stream)
            stream = 0
        }
    }
}

internal object SchemaStreams {
    /** Makes outgoing stream values lowerable (see `OutgoingStreams`). Idempotent; called by the entry points that can lower them. */
    fun install() {
        cloud.golem.wasm.OutgoingStreams.factory = ::createOutgoing
    }

    /** Creates a native stream, wraps its readable end into a `schema-value-stream` handle, and queues [produce] to run after `task.return`. */
    @Suppress("TooGenericExceptionCaught") // a failing producer must still finish the sink; VM errors are not caught
    fun createOutgoing(produce: (SchemaStreamSink) -> Unit): Int {
        val packed = hostWrapStreamNew() // readable | writable << 32
        val readable = (packed and 0xFFFFFFFFL).toInt()
        val writable = (packed ushr 32).toInt()
        val ret = alloc(4, 4)
        awaitSubtask(hostWrap(readable, ret))
        val handle = loadInt(ret)
        var open = true
        val sink = SchemaStreamSink(
            writeElement = { value ->
                if (!open) {
                    false
                } else {
                    val element = buildSchemaValueTree(value) // a 12-byte schema-value-tree record, exactly one stream element
                    val status = awaitOperation(writable, hostWrapStreamWrite(writable, element, 1))
                    if (StreamStatus.code(status) != StreamStatus.COMPLETED) open = false
                    // The element was consumed: nothing lowered for it is needed any more (the producer runs after the invocation's own
                    // memory was released, so this keeps a long stream inside the linear-memory arena).
                    if (AsyncJobs.reclaimMemory) resetHeap()
                    StreamStatus.count(status) == 1
                }
            },
            finishStream = {
                hostWrapStreamDropWritable(writable)
                open = false
            },
        )
        AsyncJobs.after {
            try {
                produce(sink)
            } catch (e: Exception) {
                // The reader sees the stream end; there is no channel left to report the failure on.
                Logging.error("outgoing stream producer failed", e, "golem.stream")
            } finally {
                sink.finish()
            }
        }
        return handle
    }
}
