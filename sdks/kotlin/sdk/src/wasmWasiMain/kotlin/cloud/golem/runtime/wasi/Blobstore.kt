@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.wasi

import cloud.golem.runtime.Either
import cloud.golem.wasm.alloc
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
// `Container.writeData` is the deepest resource chain built so far: blobstore's own
// `outgoing-value` resource -> `outgoing-value-write-body()` returns a wasi:io `output-stream`
// resource (a DIFFERENT package) -> `blocking-write-and-flush` can fail with a `stream-error`
// variant whose one payload case wraps YET ANOTHER resource, wasi:io/error's `error`
// (`to-debug-string(): string`). wasi:io/streams@0.2.3 and wasi:io/error@0.2.3 are already
// part of this world's import surface (present in every build since before this SDK added any
// custom host API -- likely pulled in by the Kotlin/Wasm runtime's own stdout/stderr support),
// so no wit-native/main.wit edit was needed for those two; only the wasi:blobstore interfaces
// themselves needed adding.

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[static]outgoing-value.new-outgoing-value")
private external fun hostNewOutgoingValue(): Int

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[method]outgoing-value.outgoing-value-write-body")
private external fun hostOutgoingValueWriteBody(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[resource-drop]outgoing-value")
private external fun hostOutgoingValueDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[method]incoming-value.incoming-value-consume-sync")
private external fun hostIncomingValueConsumeSync(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/types", "[resource-drop]incoming-value")
private external fun hostIncomingValueDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:io/streams@0.2.3", "[method]output-stream.blocking-write-and-flush")
private external fun hostOutputStreamBlockingWriteAndFlush(handle: Int, dataPtr: Int, dataLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:io/streams@0.2.3", "[resource-drop]output-stream")
private external fun hostOutputStreamDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:io/error@0.2.3", "[method]error.to-debug-string")
private external fun hostIoErrorToDebugString(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:io/error@0.2.3", "[resource-drop]error")
private external fun hostIoErrorDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.name")
private external fun hostContainerName(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.info")
private external fun hostContainerInfo(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.get-data")
private external fun hostContainerGetData(handle: Int, namePtr: Int, nameLen: Int, start: Long, end: Long, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.write-data")
private external fun hostContainerWriteData(handle: Int, namePtr: Int, nameLen: Int, outgoingValueHandle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]container.list-objects")
private external fun hostContainerListObjects(handle: Int, retPtr: Int)

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

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[method]stream-object-names.read-stream-object-names")
private external fun hostStreamObjectNamesRead(handle: Int, len: Long, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:blobstore/container", "[resource-drop]stream-object-names")
private external fun hostStreamObjectNamesDrop(handle: Int)

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

private fun liftListOfString(base: Int): List<String> {
    val dataPtr = loadInt(base)
    val len = loadInt(base + 4)
    return (0 until len).map { i ->
        val elemPtr = dataPtr + i * 8
        liftString(loadInt(elemPtr), loadInt(elemPtr + 4))
    }
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

// Resolves a wasi:io stream-error into a message: last-operation-failed wraps a wasi:io/error
// resource (resolved via to-debug-string, then dropped); closed has no payload.
private fun liftStreamErrorMessage(base: Int): String {
    val tag = loadByte(base).toInt() and 0xFF
    return if (tag == 0) {
        val errHandle = loadInt(base + 4)
        val retPtr = alloc(8, 4)
        hostIoErrorToDebugString(errHandle, retPtr)
        val msg = liftString(loadInt(retPtr), loadInt(retPtr + 4))
        hostIoErrorDrop(errHandle)
        msg
    } else {
        "stream closed"
    }
}

// Writes `data` to a fresh outgoing-value's output-stream and returns its handle for the
// caller to pass (borrowed) to container.write-data, or an error message. The outgoing-value
// itself is NOT dropped here -- write-data still needs to borrow it; the caller drops it after.
private fun writeOutgoingValue(data: ByteArray): Either<String, Int> {
    val ovHandle = hostNewOutgoingValue()
    val streamRetPtr = alloc(8, 4) // result<output-stream>: tag@0(1,1), payload@4(i32 handle or nothing)
    hostOutgoingValueWriteBody(ovHandle, streamRetPtr)
    if (loadByte(streamRetPtr).toInt() != 0) {
        hostOutgoingValueDrop(ovHandle)
        return Either.Left("failed to open output-stream for outgoing-value")
    }
    val streamHandle = loadInt(streamRetPtr + 4)
    val dataPtr = alloc(data.size, 1)
    data.forEachIndexed { i, b -> storeByte(dataPtr + i, b) }
    val writeRetPtr = alloc(12, 4) // result<_, stream-error>: tag@0(1,1), payload@4(8,4)
    hostOutputStreamBlockingWriteAndFlush(streamHandle, dataPtr, data.size, writeRetPtr)
    hostOutputStreamDrop(streamHandle)
    return if (loadByte(writeRetPtr).toInt() == 0) {
        Either.Right(ovHandle)
    } else {
        val msg = liftStreamErrorMessage(writeRetPtr + 4)
        hostOutgoingValueDrop(ovHandle)
        Either.Left(msg)
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

    fun writeData(objectName: String, data: ByteArray): Either<String, Unit> {
        check(!closed) { "Container already closed" }
        val (namePtr, nameLen) = lowerStringToPtrLen(objectName)
        val ov = writeOutgoingValue(data)
        if (ov is Either.Left) return ov
        val ovHandle = (ov as Either.Right).value
        val retPtr = alloc(12, 4) // result<_, error>
        hostContainerWriteData(handle, namePtr, nameLen, ovHandle, retPtr)
        hostOutgoingValueDrop(ovHandle)
        return liftBlobResult(retPtr, 4) { }
    }

    /**
     * Names of objects in the container. Mirrors the Scala reference's own limitation: reads a
     * single batch of up to 1000 names and does not paginate further (Scala's `listObjects`
     * does the same -- calls `readStreamObjectNames(1000)` once and returns just that batch,
     * discarding the "end of stream" flag). Not a full listing for containers with >1000 objects.
     */
    fun listObjects(): Either<String, List<String>> {
        check(!closed) { "Container already closed" }
        val retPtr = alloc(12, 4) // result<stream-object-names, error>
        hostContainerListObjects(handle, retPtr)
        val sonResult = liftBlobResult(retPtr, 4) { loadInt(it) }
        if (sonResult is Either.Left) return sonResult
        val sonHandle = (sonResult as Either.Right).value
        val readRetPtr = alloc(16, 4) // result<tuple<list<object-name>, bool>, error>
        hostStreamObjectNamesRead(sonHandle, 1000L, readRetPtr)
        hostStreamObjectNamesDrop(sonHandle)
        return liftBlobResult(readRetPtr, 4) { liftListOfString(it) }
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
