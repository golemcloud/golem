@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.host

import cloud.golem.runtime.AgentId
import cloud.golem.runtime.Either
import cloud.golem.runtime.lowerStringToPtrLen
import cloud.golem.wasm.alloc

// Native SDK access to golem:api/oplog@1.5.0's read surface -- the `get-oplog` and `search-oplog`
// resources, mirroring the Scala SDK's `OplogApi.GetOplog`/`SearchOplog`. Both yield
// `public-oplog-entry`, a ~50-case variant whose Kotlin model and canonical-ABI decoder are GENERATED
// from the WIT by scripts/gen-oplog.py into OplogEntries.kt (offsets and tags are never hand-derived).

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[constructor]get-oplog")
private external fun hostGetOplogNew(uuidHigh: Long, uuidLow: Long, idPtr: Int, idLen: Int, start: Long): Int

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[method]get-oplog.get-next")
private external fun hostGetOplogGetNext(self: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[resource-drop]get-oplog")
private external fun hostGetOplogDrop(handle: Int)

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[constructor]search-oplog")
private external fun hostSearchOplogNew(uuidHigh: Long, uuidLow: Long, idPtr: Int, idLen: Int, textPtr: Int, textLen: Int): Int

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[method]search-oplog.get-next")
private external fun hostSearchOplogGetNext(self: Int, retPtr: Int)

@kotlin.wasm.WasmImport("golem:api/oplog@1.5.0", "[resource-drop]search-oplog")
private external fun hostSearchOplogDrop(handle: Int)

/** WIT `timestamp` (an `instant`: seconds since the Unix epoch + sub-second nanos). */
data class Timestamp(val seconds: Long, val nanoseconds: Int)

/** Thrown when the host rejects an oplog read; [error] is the WIT `oplog-read-error`. */
class OplogReadException(val error: OplogReadError) :
    RuntimeException(
        when (error) {
            is OplogReadError.PermissionDenied -> "oplog read denied: permission denied"
            is OplogReadError.InternalError -> "oplog read failed: ${error.value}"
        },
    )

private fun <T> Either<OplogReadError, T>.orThrow(): T = when (this) {
    is Either.Right -> value
    is Either.Left -> throw OplogReadException(value)
}

/**
 * Reads an agent's oplog forward from a starting index. Mirrors the Scala SDK's
 * `OplogApi.GetOplog`. [close] when done -- the handle is not tied to Kotlin/Wasm GC.
 */
class GetOplog(agentId: AgentId, start: Long) {
    private val handle: Int

    init {
        val (idPtr, idLen) = lowerStringToPtrLen(agentId.agentId)
        handle = hostGetOplogNew(
            agentId.componentId.uuid.highBits,
            agentId.componentId.uuid.lowBits,
            idPtr,
            idLen,
            start,
        )
    }

    /**
     * The next batch of entries, or null once the oplog is exhausted.
     * @throws OplogReadException if the host refuses the read (for example permission denied).
     */
    fun getNext(): List<PublicOplogEntry>? {
        val ret = alloc(GET_OPLOG_NEXT_SIZE, GET_OPLOG_NEXT_ALIGN)
        hostGetOplogGetNext(handle, ret)
        return liftGetOplogNext(ret).orThrow()
    }

    private var closed = false

    /** Releases the get-oplog handle's guest-side handle-table entry. */
    fun close() {
        if (closed) return
        closed = true
        hostGetOplogDrop(handle)
    }
}

/**
 * Full-text search over an agent's oplog. Mirrors the Scala SDK's `OplogApi.SearchOplog`. Each
 * result pairs the matching entry's oplog index with the entry. [close] when done.
 */
class SearchOplog(agentId: AgentId, text: String) {
    private val handle: Int

    init {
        val (idPtr, idLen) = lowerStringToPtrLen(agentId.agentId)
        val (textPtr, textLen) = lowerStringToPtrLen(text)
        handle = hostSearchOplogNew(
            agentId.componentId.uuid.highBits,
            agentId.componentId.uuid.lowBits,
            idPtr,
            idLen,
            textPtr,
            textLen,
        )
    }

    /**
     * The next batch of (oplog-index, entry) matches, or null once exhausted.
     * @throws OplogReadException if the host refuses the read (for example permission denied).
     */
    fun getNext(): List<Pair<Long, PublicOplogEntry>>? {
        val ret = alloc(SEARCH_OPLOG_NEXT_SIZE, SEARCH_OPLOG_NEXT_ALIGN)
        hostSearchOplogGetNext(handle, ret)
        return liftSearchOplogNext(ret).orThrow()
    }

    private var closed = false

    /** Releases the search-oplog handle's guest-side handle-table entry. */
    fun close() {
        if (closed) return
        closed = true
        hostSearchOplogDrop(handle)
    }
}
