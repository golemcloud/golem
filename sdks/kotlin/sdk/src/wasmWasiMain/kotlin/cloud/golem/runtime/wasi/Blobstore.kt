@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.wasi

import cloud.golem.runtime.Cap
import cloud.golem.runtime.Either
import cloud.golem.runtime.Tally
import cloud.golem.wasm.StallGuard
import cloud.golem.wasm.StreamStatus
import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitOperation
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt

// Raw canonical-ABI bindings to wasi:blobstore's types/container/blobstore interfaces (the
// subset Scala's Blobstore.scala wraps). Package is UNVERSIONED (`package wasi:blobstore;`,
// confirmed in wit-native/deps/blobstore/*.wit) -- same situation as wasi:logging (see
// runtime/wasi/Logging.kt); every raw module string below has no @version suffix. Signatures
// verified via abi-dump's `sig`/`resulttype` modes.
//
// Unlike wasi:keyvalue's resource-typed error, wasi:blobstore's `error` is a plain
// `type error = string;` -- one less resource to manage, Either<String, T> throughout.
//
// Golem 1.6.0 moved wasi:blobstore to WASI P3: `outgoing-value.outgoing-value-write-body` consumes a
// `stream<u8>` and `container.list-objects` returns a `stream<object-name>`. Both are driven with the
// async-lowered stream intrinsics plus a blocking wait (see cloud.golem.wasm.Streams), so [Container.writeData]
// and [Container.listObjects] work from the SDK's sync-lifted exports. Reads via `get-data` +
// `incoming-value-consume-sync` (a plain `list<u8>`) are unchanged.
//
// Stream intrinsic names are mangled by wit-component: `[stream-new-N]fn`, `[async-lower][stream-write-N]fn`,
// `[async-lower][stream-read-N]fn`, `[stream-drop-writable-N]fn`, `[stream-drop-readable-N]fn`, where `fn` is the
// full WIT function name (including `[method]resource.`) and `N` is the stream's index in that function's signature.
private const val WRITE_BODY = "[method]outgoing-value.outgoing-value-write-body"
private const val LIST_OBJECTS = "[method]container.list-objects"

// Largest slice of an object body copied into linear memory at once.
private const val WRITE_CHUNK = 64 * 1024

// Names read from `list-objects` per stream read (each element is a (ptr, len) pair, 8 bytes).
private const val LIST_BATCH = 64

/** The most object names one `listObjects` call returns; a container with more is reported as an error instead of exhausting memory. */
private const val LIST_NAMES_MAX = 100_000

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[method]incoming-value.incoming-value-consume-sync")
private external fun hostIncomingValueConsumeSync(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[resource-drop]incoming-value")
private external fun hostIncomingValueDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[static]outgoing-value.new-outgoing-value")
private external fun hostNewOutgoingValue(): Int

@kotlin.wasm.WasmImport("wasi:blobstore/types", WRITE_BODY)
private external fun hostOutgoingValueWriteBody(handle: Int, readableStream: Int): Int

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[stream-new-0]$WRITE_BODY")
private external fun hostWriteBodyStreamNew(): Long

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[async-lower][stream-write-0]$WRITE_BODY")
private external fun hostWriteBodyStreamWrite(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[stream-drop-writable-0]$WRITE_BODY")
private external fun hostWriteBodyStreamDropWritable(stream: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[resource-drop]outgoing-value")
private external fun hostOutgoingValueDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.write-data")
private external fun hostContainerWriteData(handle: Int, namePtr: Int, nameLen: Int, outgoingValue: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", LIST_OBJECTS)
private external fun hostContainerListObjects(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[async-lower][stream-read-0]$LIST_OBJECTS")
private external fun hostListObjectsStreamRead(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[stream-drop-readable-0]$LIST_OBJECTS")
private external fun hostListObjectsStreamDropReadable(stream: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.name")
private external fun hostContainerName(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.info")
private external fun hostContainerInfo(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.get-data")
private external fun hostContainerGetData(handle: Int, namePtr: Int, nameLen: Int, start: Long, end: Long, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.delete-object")
private external fun hostContainerDeleteObject(handle: Int, namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.delete-objects")
private external fun hostContainerDeleteObjects(handle: Int, namesPtr: Int, namesLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.has-object")
private external fun hostContainerHasObject(handle: Int, namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.object-info")
private external fun hostContainerObjectInfo(handle: Int, namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.clear")
private external fun hostContainerClear(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[resource-drop]container")
private external fun hostContainerDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/blobstore", "create-container")
private external fun hostCreateContainer(namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/blobstore", "get-container")
private external fun hostGetContainer(namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/blobstore", "delete-container")
private external fun hostDeleteContainer(namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/blobstore", "container-exists")
private external fun hostContainerExists(namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/blobstore", "copy-object")
private external fun hostCopyObject(
    srcContainerPtr: Int,
    srcContainerLen: Int,
    srcObjectPtr: Int,
    srcObjectLen: Int,
    destContainerPtr: Int,
    destContainerLen: Int,
    destObjectPtr: Int,
    destObjectLen: Int,
    retPtr: Int,
)

@kotlin.wasm.WasmImport("wasi:blobstore/blobstore", "move-object")
private external fun hostMoveObject(
    srcContainerPtr: Int,
    srcContainerLen: Int,
    srcObjectPtr: Int,
    srcObjectLen: Int,
    destContainerPtr: Int,
    destContainerLen: Int,
    destObjectPtr: Int,
    destObjectLen: Int,
    retPtr: Int,
)

data class ContainerMetadata(val name: String, val createdAt: Long)
data class ObjectMetadata(val name: String, val container: String, val createdAt: Long, val size: Long)
data class ObjectId(val container: String, val name: String)

private fun lowerStringToPtrLen(s: String): Pair<Int, Int> {
    val bytes = s.encodeToByteArray()
    val ptr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(ptr + i, bytes[i])
    return ptr to bytes.size
}

private fun lowerListOfStringToPtrLen(items: List<String>): Pair<Int, Int> {
    val arr = alloc(items.size * 8, 4)
    items.forEachIndexed { i, s ->
        val (ptr, len) = lowerStringToPtrLen(s)
        storeInt(arr + i * 8, ptr)
        storeInt(arr + i * 8 + 4, len)
    }
    return arr to items.size
}

/** `result<T, error>` where `error` is `wasi:blobstore`'s plain `type error = string;`. [payloadOffset] varies per function (4 or 8, depending on whether the ok payload needs 8-byte alignment). */
private fun <T> liftBlobResult(base: Int, payloadOffset: Int, liftOk: (Int) -> T): Either<String, T> {
    val payload = base + payloadOffset
    return if (loadByte(base).toInt() == 0) {
        Either.Right(liftOk(payload))
    } else {
        Either.Left(liftString(loadInt(payload), loadInt(payload + 4)))
    }
}

/** A collection of objects (`wasi:blobstore/container`'s `container` resource). MUST be [close]d when done. */
class Container internal constructor(private val handle: Int) {
    private var closed = false

    fun name(): Either<String, String> {
        check(!closed) { "Container already closed" }
        val retPtr = alloc(12, 4)
        hostContainerName(handle, retPtr)
        return liftBlobResult(retPtr, 4) { liftString(loadInt(it), loadInt(it + 4)) }
    }

    fun info(): Either<String, ContainerMetadata> {
        check(!closed) { "Container already closed" }
        val retPtr = alloc(24, 8) // container-metadata: name@0(8,4), created-at@8(8,8) -> size16,align8
        hostContainerInfo(handle, retPtr)
        return liftBlobResult(retPtr, 8) { b -> ContainerMetadata(liftString(loadInt(b), loadInt(b + 4)), loadLong(b + 8)) }
    }

    fun getData(objectName: String, start: Long, end: Long): Either<String, ByteArray> {
        check(!closed) { "Container already closed" }
        val (namePtr, nameLen) = lowerStringToPtrLen(objectName)
        val retPtr = alloc(12, 4) // result<incoming-value, error>
        hostContainerGetData(handle, namePtr, nameLen, start, end, retPtr)
        return liftBlobResult(retPtr, 4) { consumeIncomingValue(loadInt(it)) }
    }

    /**
     * Creates or replaces [objectName] with [data]. The body travels to the host as a `stream<u8>` (Golem 1.6.0).
     * An empty [data] creates an empty object.
     */
    fun writeData(objectName: String, data: ByteArray): Either<String, Unit> {
        check(!closed) { "Container already closed" }
        val outgoing = hostNewOutgoingValue()
        try {
            val packed = hostWriteBodyStreamNew()
            val readable = (packed and 0xFFFFFFFFL).toInt()
            val writable = (packed ushr 32).toInt()
            // The host takes ownership of the readable end and drains it concurrently.
            if (hostOutgoingValueWriteBody(outgoing, readable) != 0) {
                hostWriteBodyStreamDropWritable(writable)
                return Either.Left("outgoing-value-write-body failed")
            }
            val buffer = alloc(minOf(data.size, WRITE_CHUNK).coerceAtLeast(1), 1)
            var sent = 0
            var open = true
            val stall = StallGuard("blobstore write")
            try {
                while (open && sent < data.size) {
                    val chunk = minOf(WRITE_CHUNK, data.size - sent)
                    for (i in 0 until chunk) storeByte(buffer + i, data[sent + i])
                    var done = 0
                    while (done < chunk) {
                        val status = awaitOperation(writable, hostWriteBodyStreamWrite(writable, buffer + done, chunk - done))
                        done += StreamStatus.count(status)
                        stall.record(StreamStatus.count(status) > 0 || StreamStatus.code(status) != StreamStatus.COMPLETED)
                        if (StreamStatus.code(status) != StreamStatus.COMPLETED) {
                            open = false
                            break
                        }
                    }
                    sent += done
                }
            } finally {
                // Closing the writable end tells the host the body is complete.
                hostWriteBodyStreamDropWritable(writable)
            }
            if (sent < data.size) return Either.Left("blobstore closed the body stream after $sent of ${data.size} bytes")
            val (namePtr, nameLen) = lowerStringToPtrLen(objectName)
            val retPtr = alloc(12, 4) // result<_, error>
            hostContainerWriteData(handle, namePtr, nameLen, outgoing, retPtr)
            return liftBlobResult(retPtr, 4) { }
        } finally {
            hostOutgoingValueDrop(outgoing)
        }
    }

    /** The names of all objects in this container, in the host's (undefined) order. Read from a `stream<object-name>`. */
    fun listObjects(maxNames: Int = LIST_NAMES_MAX): Either<String, List<String>> {
        Cap.requireLimit(maxNames, LIST_NAMES_MAX, "maxNames")
        check(!closed) { "Container already closed" }
        val retPtr = alloc(12, 4) // result<stream<object-name>, error>
        hostContainerListObjects(handle, retPtr)
        if (loadByte(retPtr).toInt() != 0) return Either.Left(liftString(loadInt(retPtr + 4), loadInt(retPtr + 8)))
        val stream = loadInt(retPtr + 4)
        val tally = Tally(maxNames)
        val names = mutableListOf<String>()
        val buffer = alloc(LIST_BATCH * 8, 4)
        val stall = StallGuard("blobstore list")
        try {
            while (true) {
                val status = awaitOperation(stream, hostListObjectsStreamRead(stream, buffer, LIST_BATCH))
                stall.record(StreamStatus.count(status) > 0 || StreamStatus.code(status) != StreamStatus.COMPLETED)
                if (!tally.add(StreamStatus.count(status))) return Either.Left("the container holds more than $maxNames objects")
                for (i in 0 until StreamStatus.count(status)) {
                    names.add(liftString(loadInt(buffer + i * 8), loadInt(buffer + i * 8 + 4)))
                }
                if (StreamStatus.code(status) != StreamStatus.COMPLETED) break
            }
        } finally {
            hostListObjectsStreamDropReadable(stream)
        }
        return Either.Right(names)
    }

    fun deleteObject(name: String): Either<String, Unit> {
        check(!closed) { "Container already closed" }
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val retPtr = alloc(12, 4)
        hostContainerDeleteObject(handle, namePtr, nameLen, retPtr)
        return liftBlobResult(retPtr, 4) { }
    }

    fun deleteObjects(names: List<String>): Either<String, Unit> {
        check(!closed) { "Container already closed" }
        val (namesPtr, namesLen) = lowerListOfStringToPtrLen(names)
        val retPtr = alloc(12, 4)
        hostContainerDeleteObjects(handle, namesPtr, namesLen, retPtr)
        return liftBlobResult(retPtr, 4) { }
    }

    fun hasObject(name: String): Either<String, Boolean> {
        check(!closed) { "Container already closed" }
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val retPtr = alloc(12, 4)
        hostContainerHasObject(handle, namePtr, nameLen, retPtr)
        return liftBlobResult(retPtr, 4) { loadByte(it).toInt() != 0 }
    }

    fun objectInfo(name: String): Either<String, ObjectMetadata> {
        check(!closed) { "Container already closed" }
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        // object-metadata: name@0(8,4), container@8(8,4), created-at@16(8,8), size@24(8,8) -> size32,align8.
        val retPtr = alloc(40, 8)
        hostContainerObjectInfo(handle, namePtr, nameLen, retPtr)
        return liftBlobResult(retPtr, 8) { b ->
            ObjectMetadata(
                name = liftString(loadInt(b), loadInt(b + 4)),
                container = liftString(loadInt(b + 8), loadInt(b + 12)),
                createdAt = loadLong(b + 16),
                size = loadLong(b + 24),
            )
        }
    }

    fun clear(): Either<String, Unit> {
        check(!closed) { "Container already closed" }
        val retPtr = alloc(12, 4)
        hostContainerClear(handle, retPtr)
        return liftBlobResult(retPtr, 4) { }
    }

    fun close() {
        if (!closed) {
            hostContainerDrop(handle)
            closed = true
        }
    }
}

// Consumes and drops an incoming-value handle in one step, same pattern as KeyValue.kt's
// consumeIncomingValue -- incoming-value isn't exposed as public API.
private fun consumeIncomingValue(handle: Int): ByteArray {
    val retPtr = alloc(12, 4) // result<list<u8>, error>
    hostIncomingValueConsumeSync(handle, retPtr)
    val result = liftBlobResult(retPtr, 4) { b ->
        val dataPtr = loadInt(b)
        val len = loadInt(b + 4)
        ByteArray(len) { i -> loadByte(dataPtr + i) }
    }
    hostIncomingValueDrop(handle)
    return when (result) {
        is Either.Right -> result.value
        is Either.Left -> error("incoming-value-consume-sync failed: ${result.value}")
    }
}

/** Native SDK access to `wasi:blobstore`. Mirrors the Scala SDK's `Blobstore` object. */
object Blobstore {
    fun createContainer(name: String): Either<String, Container> {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val retPtr = alloc(12, 4)
        hostCreateContainer(namePtr, nameLen, retPtr)
        return liftBlobResult(retPtr, 4) { Container(loadInt(it)) }
    }

    fun getContainer(name: String): Either<String, Container> {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val retPtr = alloc(12, 4)
        hostGetContainer(namePtr, nameLen, retPtr)
        return liftBlobResult(retPtr, 4) { Container(loadInt(it)) }
    }

    fun deleteContainer(name: String): Either<String, Unit> {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val retPtr = alloc(12, 4)
        hostDeleteContainer(namePtr, nameLen, retPtr)
        return liftBlobResult(retPtr, 4) { }
    }

    fun containerExists(name: String): Either<String, Boolean> {
        val (namePtr, nameLen) = lowerStringToPtrLen(name)
        val retPtr = alloc(12, 4)
        hostContainerExists(namePtr, nameLen, retPtr)
        return liftBlobResult(retPtr, 4) { loadByte(it).toInt() != 0 }
    }

    fun copyObject(src: ObjectId, dest: ObjectId): Either<String, Unit> {
        val (srcContainerPtr, srcContainerLen) = lowerStringToPtrLen(src.container)
        val (srcObjectPtr, srcObjectLen) = lowerStringToPtrLen(src.name)
        val (destContainerPtr, destContainerLen) = lowerStringToPtrLen(dest.container)
        val (destObjectPtr, destObjectLen) = lowerStringToPtrLen(dest.name)
        val retPtr = alloc(12, 4)
        hostCopyObject(
            srcContainerPtr, srcContainerLen, srcObjectPtr, srcObjectLen,
            destContainerPtr, destContainerLen, destObjectPtr, destObjectLen,
            retPtr,
        )
        return liftBlobResult(retPtr, 4) { }
    }

    fun moveObject(src: ObjectId, dest: ObjectId): Either<String, Unit> {
        val (srcContainerPtr, srcContainerLen) = lowerStringToPtrLen(src.container)
        val (srcObjectPtr, srcObjectLen) = lowerStringToPtrLen(src.name)
        val (destContainerPtr, destContainerLen) = lowerStringToPtrLen(dest.container)
        val (destObjectPtr, destObjectLen) = lowerStringToPtrLen(dest.name)
        val retPtr = alloc(12, 4)
        hostMoveObject(
            srcContainerPtr, srcContainerLen, srcObjectPtr, srcObjectLen,
            destContainerPtr, destContainerLen, destObjectPtr, destObjectLen,
            retPtr,
        )
        return liftBlobResult(retPtr, 4) { }
    }
}
