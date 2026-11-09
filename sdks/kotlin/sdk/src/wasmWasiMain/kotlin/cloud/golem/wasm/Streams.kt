@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.wasm

// Blocking helper for component-model `stream<T>` / `future<T>` operations (WASI P3).
//
// The exports of this SDK are sync-lifted, so there is no event loop to return to. Streams are still
// usable: each stream operation is imported *async-lowered* (`[async-lower][stream-read-N]fn` /
// `[async-lower][stream-write-N]fn`), which either completes at once or returns BLOCKED, and a BLOCKED
// operation is then waited out with `waitable-set.wait` right here. (The *sync-lowered* forms would be
// simpler but need the `cm-more-async-builtins` feature, which the Golem executor does not enable.)
// Verified against the 1.6.0 executor with wasi:blobstore (see the 1.6.0 update plan, streams spike).

@kotlin.wasm.WasmImport("\$root", "[waitable-set-new]")
private external fun hostWaitableSetNew(): Int

@kotlin.wasm.WasmImport("\$root", "[waitable-set-wait]")
private external fun hostWaitableSetWait(set: Int, retPtr: Int): Int

@kotlin.wasm.WasmImport("\$root", "[waitable-join]")
private external fun hostWaitableJoin(waitable: Int, set: Int)

@kotlin.wasm.WasmImport("\$root", "[waitable-set-drop]")
private external fun hostWaitableSetDrop(set: Int)

@kotlin.wasm.WasmImport("\$root", "[subtask-drop]")
private external fun hostSubtaskDrop(task: Int)

/** The packed status word an async-lowered stream/future operation returns: `result | count << 4`. */
/** Consecutive stream completions that moved no element before a loop treats the host as stuck. */
internal const val STALLED_OPERATIONS_LIMIT = 1_000

/**
 * Fails a loop that keeps completing stream operations without moving any element, instead of spinning forever. Call [record] once
 * per completed operation with whether it made progress.
 */
internal class StallGuard(private val what: String) {
    private var stalled = 0

    fun record(progressed: Boolean) {
        stalled = if (progressed) 0 else stalled + 1
        check(stalled <= STALLED_OPERATIONS_LIMIT) { "$what: $stalled consecutive stream operations moved no element" }
    }
}

internal object StreamStatus {
    /** The operation did not finish; wait for it with [awaitOperation]. */
    const val BLOCKED = -1

    /** [code]: the requested transfer finished (possibly fewer elements than requested). */
    const val COMPLETED = 0

    /** [code]: the other end was dropped (end of stream, or the reader went away). */
    const val DROPPED = 1

    /** [code]: the operation was cancelled. */
    const val CANCELLED = 2

    fun code(status: Int): Int = status and 0xF

    /** Number of elements transferred. */
    fun count(status: Int): Int = status ushr 4
}

/**
 * Returns the final packed status of an async-lowered stream/future operation on [handle] that returned
 * [status]: [status] itself when it already finished, otherwise the status delivered by the wait event.
 */
internal fun awaitOperation(handle: Int, status: Int): Int {
    if (status != StreamStatus.BLOCKED) return status
    val set = hostWaitableSetNew()
    try {
        hostWaitableJoin(handle, set)
        val event = alloc(8, 4) // { waitable index, payload }
        hostWaitableSetWait(set, event)
        hostWaitableJoin(handle, 0)
        check(loadInt(event) == handle) { "native awaitOperation: woke for waitable ${loadInt(event)}, expected $handle" }
        return loadInt(event + 4)
    } finally {
        hostWaitableSetDrop(set)
    }
}

/** Subtask states of the packed status an async-lowered *function call* returns: `state | subtask << 4`. */
internal object SubtaskState {
    const val STARTING = 0
    const val STARTED = 1
    const val RETURNED = 2
}

/**
 * Waits for the async-lowered function call that returned [status] to finish (its result is then in the
 * result area the caller passed). A call that returned `RETURNED` at once carries no subtask handle; otherwise
 * the subtask is waited on until it returns and then dropped. Verified against a real `tool-rpc.invoke-and-await`
 * (call returned STARTED, `waitable-set.wait` delivered RETURNED).
 */
internal fun awaitSubtask(status: Int) {
    var state = status and 0xF
    val task = status ushr 4
    if (state == SubtaskState.RETURNED && task == 0) return
    check(task != 0) { "native awaitSubtask: state $state without a subtask handle" }
    val set = hostWaitableSetNew()
    try {
        hostWaitableJoin(task, set)
        val event = alloc(8, 4) // { waitable index, payload = new subtask state }
        while (state < SubtaskState.RETURNED) {
            hostWaitableSetWait(set, event)
            check(loadInt(event) == task) { "native awaitSubtask: woke for waitable ${loadInt(event)}, expected $task" }
            state = loadInt(event + 4)
        }
        check(state == SubtaskState.RETURNED) { "native awaitSubtask: subtask ended in state $state (cancelled?)" }
    } finally {
        hostWaitableJoin(task, 0)
        hostWaitableSetDrop(set)
        if (state >= SubtaskState.RETURNED) hostSubtaskDrop(task)
    }
}

/**
 * A waitable set for code that has several async operations outstanding at once (a tool call with stdin, stdout
 * and stderr in flight): [join] each pending stream/subtask handle, then [wait] for whichever finishes next.
 */
internal class WaitSet {
    private val set = hostWaitableSetNew()
    private val event = alloc(8, 4) // { waitable index, payload }

    fun join(handle: Int) = hostWaitableJoin(handle, set)

    fun unjoin(handle: Int) = hostWaitableJoin(handle, 0)

    /** Blocks until a joined waitable has an event; returns its handle, with the payload in [lastPayload]. */
    fun wait(): Int {
        hostWaitableSetWait(set, event)
        return loadInt(event)
    }

    /** The payload of the event [wait] last returned (a stream status, or a subtask state). */
    val lastPayload: Int get() = loadInt(event + 4)

    fun close() = hostWaitableSetDrop(set)
}

/** Drops a finished subtask handle (see [awaitSubtask]). */
internal fun dropSubtask(task: Int) = hostSubtaskDrop(task)
