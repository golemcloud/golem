@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

import cloud.golem.wasm.StallGuard
import cloud.golem.wasm.StreamStatus
import cloud.golem.wasm.WaitSet
import cloud.golem.wasm.alloc
import cloud.golem.wasm.dropSubtask
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.storeByte

// Caller-side streamed I/O of a tool call (golem:tool/host): stdin through `create-stdin`'s writer, stdout/stderr through
// `create-output`'s readers. The host requires the caller to drive every attached stream *concurrently* with the call
// (a tool that echoes its input would otherwise deadlock a caller that writes first and reads later), so the call is
// admitted with `async-invoke-and-await` and a single-threaded event loop (one waitable set over the stdin write
// subtask and the output stream reads) runs until stdin is sent and both outputs have closed; only then is the
// result awaited. Verified against the Rust `tool-streaming` provider on a real 1.6.0 executor.
private const val HOST = "golem:tool/host@0.1.0"

// create-stdin() -> tuple<own<tool-stdin-writer>, own<tool-stdin>, own<tool-stdin-closed>>: 3 handles (12 B).
@kotlin.wasm.WasmImport(HOST, "create-stdin")
private external fun hostCreateStdin(retPtr: Int)

// create-output() -> tuple<own<tool-output>, stream<byte-stream-item>>: 2 handles (8 B).
@kotlin.wasm.WasmImport(HOST, "create-output")
private external fun hostCreateOutput(retPtr: Int)

@kotlin.wasm.WasmImport(HOST, "[async-lower][stream-read-0]create-output")
private external fun hostOutputRead(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport(HOST, "[stream-drop-readable-0]create-output")
private external fun hostOutputDropReadable(stream: Int)

@kotlin.wasm.WasmImport(HOST, "[async-lower][method]tool-stdin-writer.write")
private external fun hostStdinWrite(self: Int, ptr: Int, len: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(HOST, "[async-lower][method]tool-stdin-writer.finish")
private external fun hostStdinFinish(self: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport(HOST, "[resource-drop]tool-stdin-writer")
private external fun hostStdinWriterDrop(handle: Int)

@kotlin.wasm.WasmImport(HOST, "[resource-drop]tool-stdin-closed")
private external fun hostStdinClosedDrop(handle: Int)

private const val WRITE_CHUNK = 64 * 1024 // stdin is sent in slices of at most this many bytes
private const val READ_BATCH = 8 // byte-stream-items requested per output read
private const val ITEM_SIZE = 16 // byte-stream-item = result<list<u8>, byte-stream-failure>: 16 B, align 4
private const val SUBTASK_RETURNED = 2

/** The most bytes one captured output (or one [ToolStdin.readAll]) may hold; more is cut off and reported as resource-exhausted. */
internal const val CAPTURED_BYTES_MAX = 16 * 1024 * 1024

/**
 * Collects one captured output (`stream<byte-stream-item>`). Item layout: tag@0 (0 = ok); ok: list<u8> ptr@4 / len@8;
 * err: byte-stream-failure tag@4 (0 cancelled, 1 abandoned, 2 resource-exhausted, 3 failed(string ptr@8 / len@12)).
 */
internal class OutputReader(val stream: Int, private val readStream: (stream: Int, ptr: Int, count: Int) -> Int) {
    private val buffer = alloc(ITEM_SIZE * READ_BATCH, 4)
    private val chunks = ArrayList<ByteArray>()
    private val stall = StallGuard("tool output read")
    private val captured = Tally(CAPTURED_BYTES_MAX)
    var open = true
        private set

    /** True once the readable end was released to the host (so it is released only once). */
    var dropped = false
    var pending = false
        private set
    var failure: ToolStreamFailure? = null
        private set

    /** When set, items go to the sink as they arrive instead of being collected. */
    var sink: ByteSink? = null

    /** Starts a read if none is outstanding; returns true when it finished at once (so another may be started). */
    fun start(set: WaitSet): Boolean {
        if (!open || pending) return false
        val status = readStream(stream, buffer, READ_BATCH)
        if (status == StreamStatus.BLOCKED) {
            pending = true
            set.join(stream)
            return false
        }
        finish(status)
        return true
    }

    /** Applies the final packed status of a read (items transferred + whether the stream ended). */
    fun finish(status: Int) {
        pending = false
        stall.record(StreamStatus.count(status) > 0 || StreamStatus.code(status) != StreamStatus.COMPLETED)
        for (i in 0 until StreamStatus.count(status)) readItem(buffer + i * ITEM_SIZE)
        if (StreamStatus.code(status) != StreamStatus.COMPLETED || failure is ToolStreamFailure.ResourceExhausted) open = false
    }

    private fun readItem(item: Int) {
        if (loadByte(item).toInt() and 0xFF == 0) {
            val ptr = loadInt(item + 4)
            val len = loadInt(item + 8)
            if (failure != null) return
            val forward = sink
            if (forward != null) {
                if (!forward.write(ByteArray(len) { loadByte(ptr + it) })) sink = DISCARD
                return
            }
            if (!captured.add(len)) {
                // Stop collecting: the rest is dropped when the stream's readable end is released.
                failure = ToolStreamFailure.ResourceExhausted
                return
            }
            chunks.add(ByteArray(len) { loadByte(ptr + it) })
        } else {
            failure = when (loadByte(item + 4).toInt() and 0xFF) {
                0 -> ToolStreamFailure.Cancelled
                1 -> ToolStreamFailure.Abandoned
                2 -> ToolStreamFailure.ResourceExhausted
                else -> ToolStreamFailure.Failed(liftString(loadInt(item + 8), loadInt(item + 12)))
            }
        }
    }

    fun bytes(): ByteArray {
        val out = ByteArray(chunks.sumOf { it.size })
        var at = 0
        for (c in chunks) {
            c.copyInto(out, at)
            at += c.size
        }
        return out
    }
}

/**
 * Sends [data] through a `tool-stdin-writer` in slices and then finishes it. Each `write` / `finish` is an async-lowered
 * call: it either returns at once or leaves a subtask that the event loop waits on. A write error means the tool stopped
 * reading (consumer cancelled / closed), which is not a failure of the call: the rest of the input is dropped.
 */
private class StdinPump(private val writer: Int, private val data: ByteArray) {
    private val result = alloc(32, 4) // result<_, stream-write-error>: tag@0 (0 = ok)

    // One slice is in flight at a time, so one buffer serves the whole input.
    private val slice = alloc(minOf(WRITE_CHUNK, data.size).coerceAtLeast(1), 1)

    private var offset = 0
    private var inFlight = 0 // bytes of the write in flight
    private var finishing = false
    var done = false
        private set
    var task = 0 // the pending subtask handle, or 0
        private set

    /** Starts the next write / finish if none is outstanding; returns true when it finished at once. */
    fun start(set: WaitSet): Boolean {
        if (done || task != 0) return false
        val status: Int
        if (offset < data.size) {
            inFlight = minOf(WRITE_CHUNK, data.size - offset)
            for (i in 0 until inFlight) storeByte(slice + i, data[offset + i])
            status = hostStdinWrite(writer, slice, inFlight, result)
        } else {
            finishing = true
            status = hostStdinFinish(writer, result)
        }
        val state = status and 0xF
        if (state == SUBTASK_RETURNED) {
            complete()
            return true
        }
        task = status ushr 4
        set.join(task)
        return false
    }

    /** Applies a subtask state change; returns true when the subtask returned. */
    fun onEvent(set: WaitSet, state: Int): Boolean {
        if (state != SUBTASK_RETURNED) return false
        set.unjoin(task)
        dropSubtask(task)
        task = 0
        complete()
        return true
    }

    private fun complete() {
        if (loadByte(result).toInt() and 0xFF != 0) {
            done = true // closed by the tool (or the call ended): nothing more to send
        } else if (finishing) {
            done = true
        } else {
            offset += inFlight
        }
    }
}

/** Handles of the output attachments created for one call (0 = not captured). */
private class Outputs(captureStdout: Boolean, captureStderr: Boolean) {
    var stdoutTarget = 0
    var stderrTarget = 0
    val stdout: OutputReader?
    val stderr: OutputReader?

    init {
        stdout = if (captureStdout) create(true) else null
        stderr = if (captureStderr) create(false) else null
    }

    private fun create(isStdout: Boolean): OutputReader {
        val ret = alloc(8, 4)
        hostCreateOutput(ret)
        if (isStdout) stdoutTarget = loadInt(ret) else stderrTarget = loadInt(ret)
        return OutputReader(loadInt(ret + 4), ::hostOutputRead)
    }
}

/**
 * The stdin writer and captured outputs attached to one call. [source] / [stdoutTarget] / [stderrTarget] are the `own`
 * handles moved into the invocation (0 = none); [drive] then runs the caller side until stdin is sent and both outputs
 * closed, and [merge] folds the captured bytes into the call's result.
 */
internal class ToolAttachments(stdin: ByteArray?, captureStdout: Boolean, captureStderr: Boolean) {
    private var writer = 0
    var source = 0
        private set
    private val outputs = Outputs(captureStdout, captureStderr)
    private val pump: StdinPump?
    private var driven = false

    val stdoutTarget get() = outputs.stdoutTarget
    val stderrTarget get() = outputs.stderrTarget

    init {
        if (stdin != null) {
            val ret = alloc(12, 4)
            hostCreateStdin(ret)
            writer = loadInt(ret)
            source = loadInt(ret + 4)
            hostStdinClosedDrop(loadInt(ret + 8)) // the closure watcher is only for SDK pumps that block on a native source
        }
        pump = if (stdin != null) StdinPump(writer, stdin) else null
    }

    /** Runs the single-threaded event loop (one waitable set over the stdin write subtask and the output reads) to completion. */
    fun drive() {
        if (driven) return
        driven = true
        val set = WaitSet()
        try {
            val readers = listOfNotNull(outputs.stdout, outputs.stderr)
            while (true) {
                var progressed: Boolean
                do {
                    progressed = pump?.start(set) ?: false
                    for (reader in readers) {
                        if (reader.start(set)) progressed = true
                        releaseStopped(reader)
                    }
                } while (progressed)
                val stdinPending = pump != null && pump.task != 0
                if (!stdinPending && readers.none { it.pending }) break
                val handle = set.wait()
                val payload = set.lastPayload
                val reader = readers.firstOrNull { it.stream == handle }
                if (reader != null) {
                    set.unjoin(handle)
                    reader.finish(payload)
                    releaseStopped(reader)
                } else {
                    check(pump != null && handle == pump.task) { "native tool I/O: woke for unexpected waitable $handle" }
                    pump.onEvent(set, payload)
                }
            }
        } finally {
            set.close()
            if (writer != 0) hostStdinWriterDrop(writer)
            outputs.stdout?.takeIf { !it.dropped }?.let { hostOutputDropReadable(it.stream) }
            outputs.stderr?.takeIf { !it.dropped }?.let { hostOutputDropReadable(it.stream) }
        }
    }

    /**
     * Releases the readable end of an output that will not be read again, at once: because it reached its ceiling or ended.
     * Held until the call ends, the end would keep the tool's writes blocked on a full buffer, which stalls a tool that is still
     * reading its stdin, and so the stdin pump too. Once released the tool's `write` returns false and it carries on.
     */
    private fun releaseStopped(reader: OutputReader) {
        if (!reader.open && !reader.pending && !reader.dropped) {
            reader.dropped = true
            hostOutputDropReadable(reader.stream)
        }
    }

    /** Adds the captured stdout / stderr to a successful [outcome]; errors pass through. */
    fun merge(outcome: ToolInvokeResult): ToolInvokeResult = if (outcome is ToolInvokeResult.Ok) {
        ToolInvokeResult.Ok(
            outcome.value.copy(
                stdout = outputs.stdout?.bytes(),
                stderr = outputs.stderr?.bytes(),
                stdoutFailure = outputs.stdout?.failure,
                stderrFailure = outputs.stderr?.failure,
            ),
        )
    } else {
        outcome
    }
}

/** [ToolRpc.invokeAndAwait] with stdin and/or captured outputs. */
internal fun invokeWithStreams(
    rpc: Int,
    commandPath: List<String>,
    input: TypedSchemaValue,
    stdin: ByteArray?,
    captureStdout: Boolean,
    captureStderr: Boolean,
): ToolInvokeResult {
    val attachments = ToolAttachments(stdin, captureStdout, captureStderr)
    val args = buildInvokeArgs(rpc, commandPath, input, attachments.source, attachments.stdoutTarget, attachments.stderrTarget)
    val future = ToolFutureInvokeResult(hostToolRpcAsyncInvokeAndAwait(args), attachments)
    attachments.drive()
    val outcome = future.get()
    future.close()
    return outcome
}

private val DISCARD = object : ByteSink {
    override fun write(bytes: ByteArray): Boolean = false
}
