@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.wasi

import cloud.golem.runtime.Either
import cloud.golem.wasm.StallGuard
import cloud.golem.wasm.StreamStatus
import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitOperation
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt

// Raw canonical-ABI bindings to wasi:keyvalue@0.1.0's types/eventual/eventual-batch interfaces
// (the subset the Scala SDK's KeyValue.scala wraps; wasi:keyvalue also has atomic/cache/
// handle-watch interfaces this SDK doesn't cover, matching Scala's own scope). Signatures
// verified via abi-dump's `sig`/`resulttype` modes against wit-native/deps/keyvalue/*.wit.
//
// New wrinkles vs everything built so far:
//   - `error` (wasi:keyvalue/wasi-keyvalue-error) is itself a RESOURCE (a `trace(): string`
//     method), not a plain variant like wasi:config's `error` -- every `result<T, error>` err
//     case carries an owned handle that must be resolved via [method]error.trace and then
//     dropped, not decoded inline.
//   - `bucket`/`outgoing-value` are obtained via STATIC resource functions
//     (`open-bucket: static func(...)`, `new-outgoing-value: static func()`), a new intrinsic
//     name shape: `[static]<resource>.<name>` (confirmed via abi-dump's `sig` mode -- these
//     appear as ordinary `iface.functions` entries with this name, same discoverability as
//     `[constructor]`/`[method]`).
//   - `eventual`'s functions take `borrow<bucket>`/`borrow<outgoing-value>` params, not owned
//     handles -- flattens identically to `own<T>` (a plain i32), the difference being purely
//     ownership bookkeeping the canonical ABI enforces at the host side, not the wire shape.

@kotlin.wasm.WasmImport("wasi:keyvalue/wasi-keyvalue-error@0.1.0", "[method]error.trace")
private external fun hostErrorTrace(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/wasi-keyvalue-error@0.1.0", "[resource-drop]error")
private external fun hostErrorDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[static]bucket.open-bucket")
private external fun hostOpenBucket(namePtr: Int, nameLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[resource-drop]bucket")
private external fun hostBucketDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[static]outgoing-value.new-outgoing-value")
private external fun hostNewOutgoingValue(): Int

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[method]outgoing-value.outgoing-value-write-body-sync")
private external fun hostOutgoingValueWriteBodySync(handle: Int, dataPtr: Int, dataLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[resource-drop]outgoing-value")
private external fun hostOutgoingValueDrop(handle: Int)

// The async body write. The guest owns the stream: it creates the pair, hands the readable end to the host,
// writes the bytes, then drops its writable end to mark the end of the value. `[stream-*-0]` is the stream at
// index 0 of that method's signature, named the way `SchemaStreams` and `ToolIo` name theirs.
private const val WRITE_BODY_ASYNC = "[method]outgoing-value.outgoing-value-write-body-async"

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", WRITE_BODY_ASYNC)
private external fun hostOutgoingValueWriteBodyAsync(handle: Int, readable: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[stream-new-0]$WRITE_BODY_ASYNC")
private external fun hostWriteBodyStreamNew(): Long

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[async-lower][stream-write-0]$WRITE_BODY_ASYNC")
private external fun hostWriteBodyStreamWrite(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[stream-drop-writable-0]$WRITE_BODY_ASYNC")
private external fun hostWriteBodyStreamDropWritable(stream: Int)

// The async consume. The host owns the stream: it returns the readable end and the guest drains it.
private const val CONSUME_ASYNC = "[method]incoming-value.incoming-value-consume-async"

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", CONSUME_ASYNC)
private external fun hostIncomingValueConsumeAsync(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[async-lower][stream-read-0]$CONSUME_ASYNC")
private external fun hostConsumeStreamRead(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[stream-drop-readable-0]$CONSUME_ASYNC")
private external fun hostConsumeStreamDropReadable(stream: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[method]incoming-value.incoming-value-consume-sync")
private external fun hostIncomingValueConsumeSync(handle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/types@0.1.0", "[resource-drop]incoming-value")
private external fun hostIncomingValueDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/eventual@0.1.0", "get")
private external fun hostEventualGet(bucketHandle: Int, keyPtr: Int, keyLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/eventual@0.1.0", "set")
private external fun hostEventualSet(bucketHandle: Int, keyPtr: Int, keyLen: Int, outgoingValueHandle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/eventual@0.1.0", "delete")
private external fun hostEventualDelete(bucketHandle: Int, keyPtr: Int, keyLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/eventual@0.1.0", "exists")
private external fun hostEventualExists(bucketHandle: Int, keyPtr: Int, keyLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/eventual-batch@0.1.0", "get-many")
private external fun hostEventualBatchGetMany(bucketHandle: Int, keysPtr: Int, keysLen: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/eventual-batch@0.1.0", "keys")
private external fun hostEventualBatchKeys(bucketHandle: Int, retPtr: Int)

@kotlin.wasm.WasmImport("wasi:keyvalue/eventual-batch@0.1.0", "delete-many")
private external fun hostEventualBatchDeleteMany(bucketHandle: Int, keysPtr: Int, keysLen: Int, retPtr: Int)

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

/**
 * A `wasi:keyvalue` error, resolved eagerly to its trace message (and dropped) at construction
 * time -- unlike every other resource in this SDK, callers never hold onto a `KvError` handle,
 * so there's no `close()` to forget: the underlying resource is released the moment this
 * wrapper is built.
 */
class KvError {
    val message: String

    /** Wraps a real `error` resource handle returned by the host. */
    internal constructor(handle: Int) {
        val retPtr = alloc(8, 4)
        hostErrorTrace(handle, retPtr)
        message = liftString(loadInt(retPtr), loadInt(retPtr + 4))
        hostErrorDrop(handle)
    }

    /** Synthesizes an error the guest detected itself, with no host-side `error` handle to trace or drop. */
    internal constructor(message: String) {
        this.message = message
    }
}

private fun <T> liftKvResult(base: Int, liftOk: (Int) -> T): Either<KvError, T> = if (loadByte(base).toInt() == 0) Either.Right(liftOk(base + 4)) else Either.Left(KvError(loadInt(base + 4)))

private fun liftKvUnitResult(base: Int): Either<KvError, Unit> = if (loadByte(base).toInt() == 0) Either.Right(Unit) else Either.Left(KvError(loadInt(base + 4)))

/** The most bytes a single stream write offers the host, so one large value does not need one arena block per byte. */
private const val STREAM_CHUNK = 64 * 1024

/**
 * Fills a new outgoing value's body from [bytes] over a `stream<u8>` instead of one contiguous buffer, and returns
 * its handle. The caller owns the handle and must drop it.
 *
 * The guest holds the writable end: it hands the readable end to the host, writes every byte, then drops the
 * writable end. That drop is what tells the host the value is complete, so it must happen before the value is
 * stored — the bucket write below relies on it.
 */
private fun newOutgoingValueFromStream(bytes: ByteArray): Either<KvError, Int> {
    val ovHandle = hostNewOutgoingValue()
    val packed = hostWriteBodyStreamNew() // readable | writable << 32
    val readable = (packed and 0xFFFFFFFFL).toInt()
    val writable = (packed ushr 32).toInt()

    val retPtr = alloc(8, 4) // result<_, error>: tag@0, error handle@4
    hostOutgoingValueWriteBodyAsync(ovHandle, readable, retPtr)
    val accepted = liftKvUnitResult(retPtr)
    if (accepted is Either.Left) {
        hostWriteBodyStreamDropWritable(writable)
        hostOutgoingValueDrop(ovHandle)
        return accepted
    }

    val stall = StallGuard("key-value body write")
    var at = 0
    try {
        while (at < bytes.size) {
            val count = minOf(STREAM_CHUNK, bytes.size - at)
            val ptr = alloc(count, 1)
            for (i in 0 until count) storeByte(ptr + i, bytes[at + i])
            val status = awaitOperation(writable, hostWriteBodyStreamWrite(writable, ptr, count))
            val moved = StreamStatus.count(status)
            stall.record(moved > 0)
            if (StreamStatus.code(status) != StreamStatus.COMPLETED) {
                // The host stopped reading: the value cannot be completed.
                hostOutgoingValueDrop(ovHandle)
                return Either.Left(KvError("key-value body write closed after $at of ${bytes.size} bytes"))
            }
            at += moved
        }
    } finally {
        // Always: the host treats this drop as the end of the body.
        hostWriteBodyStreamDropWritable(writable)
    }
    return Either.Right(ovHandle)
}

/** Drains an incoming value over a `stream<u8>` and releases both the stream's readable end and the value. */
private fun consumeIncomingValueAsStream(ivHandle: Int): Either<KvError, ByteArray> {
    val retPtr = alloc(8, 4) // result<stream<u8>, error>: tag@0, handle@4
    hostIncomingValueConsumeAsync(ivHandle, retPtr)
    if (loadByte(retPtr).toInt() != 0) {
        val error = KvError(loadInt(retPtr + 4))
        hostIncomingValueDrop(ivHandle)
        return Either.Left(error)
    }
    val readable = loadInt(retPtr + 4)
    val out = ArrayList<ByteArray>()
    val stall = StallGuard("key-value body read")
    try {
        while (true) {
            val ptr = alloc(STREAM_CHUNK, 1)
            val status = awaitOperation(readable, hostConsumeStreamRead(readable, ptr, STREAM_CHUNK))
            val count = StreamStatus.count(status)
            if (count > 0) out.add(ByteArray(count) { loadByte(ptr + it) })
            val code = StreamStatus.code(status)
            stall.record(count > 0 || code != StreamStatus.COMPLETED)
            if (code != StreamStatus.COMPLETED) break // DROPPED is the end of the value.
        }
    } finally {
        hostConsumeStreamDropReadable(readable)
        hostIncomingValueDrop(ivHandle)
    }
    val total = out.sumOf { it.size }
    val joined = ByteArray(total)
    var at = 0
    for (chunk in out) {
        chunk.copyInto(joined, at)
        at += chunk.size
    }
    return Either.Right(joined)
}

/** A collection of key-value pairs (`wasi:keyvalue/types@0.1.0`'s `bucket` resource). MUST be [close]d when done. */
class Bucket internal constructor(private val handle: Int) {
    private var closed = false

    fun get(key: String): Either<KvError, ByteArray?> {
        check(!closed) { "Bucket already closed" }
        val (keyPtr, keyLen) = lowerStringToPtrLen(key)
        val retPtr = alloc(12, 4) // result<option<incoming-value>, error>: tag@0(1,1), payload@4(8,4)
        hostEventualGet(handle, keyPtr, keyLen, retPtr)
        return liftKvResult(retPtr) { optBase ->
            if (loadByte(optBase).toInt() == 0) null else consumeIncomingValue(loadInt(optBase + 4))
        }
    }

    fun set(key: String, value: ByteArray): Either<KvError, Unit> {
        check(!closed) { "Bucket already closed" }
        val (keyPtr, keyLen) = lowerStringToPtrLen(key)
        val ovHandle = hostNewOutgoingValue()
        val dataPtr = alloc(value.size, 1)
        value.forEachIndexed { i, b -> storeByte(dataPtr + i, b) }
        val writeRetPtr = alloc(8, 4) // result<_, error>
        hostOutgoingValueWriteBodySync(ovHandle, dataPtr, value.size, writeRetPtr)
        val writeResult = liftKvUnitResult(writeRetPtr)
        if (writeResult is Either.Left) {
            hostOutgoingValueDrop(ovHandle)
            return writeResult
        }
        val retPtr = alloc(8, 4) // result<_, error>
        hostEventualSet(handle, keyPtr, keyLen, ovHandle, retPtr)
        hostOutgoingValueDrop(ovHandle)
        return liftKvUnitResult(retPtr)
    }

    /**
     * Stores [value], streaming it to the host rather than handing over one contiguous buffer. Use this for a
     * large value: `set` copies the whole value into the linear-memory arena at once, while this moves it in
     * chunks the host drains as they arrive.
     *
     * The stored bytes are identical either way; only the transfer differs.
     */
    fun setStreaming(key: String, value: ByteArray): Either<KvError, Unit> {
        check(!closed) { "Bucket already closed" }
        val (keyPtr, keyLen) = lowerStringToPtrLen(key)
        val ovHandle = when (val created = newOutgoingValueFromStream(value)) {
            is Either.Left -> return created
            is Either.Right -> created.value
        }
        val retPtr = alloc(8, 4) // result<_, error>
        hostEventualSet(handle, keyPtr, keyLen, ovHandle, retPtr)
        hostOutgoingValueDrop(ovHandle)
        return liftKvUnitResult(retPtr)
    }

    /**
     * Reads [key], draining the value over a stream rather than receiving one contiguous buffer. Returns null
     * when the key is absent, as [get] does. Use this for a large value.
     */
    fun getStreaming(key: String): Either<KvError, ByteArray?> {
        check(!closed) { "Bucket already closed" }
        val (keyPtr, keyLen) = lowerStringToPtrLen(key)
        val retPtr = alloc(12, 4) // result<option<incoming-value>, error>: tag@0, payload@4
        hostEventualGet(handle, keyPtr, keyLen, retPtr)
        if (loadByte(retPtr).toInt() != 0) return Either.Left(KvError(loadInt(retPtr + 4)))
        val optBase = retPtr + 4
        if (loadByte(optBase).toInt() == 0) return Either.Right(null)
        return when (val drained = consumeIncomingValueAsStream(loadInt(optBase + 4))) {
            is Either.Left -> drained
            is Either.Right -> Either.Right(drained.value)
        }
    }

    fun delete(key: String): Either<KvError, Unit> {
        check(!closed) { "Bucket already closed" }
        val (keyPtr, keyLen) = lowerStringToPtrLen(key)
        val retPtr = alloc(8, 4)
        hostEventualDelete(handle, keyPtr, keyLen, retPtr)
        return liftKvUnitResult(retPtr)
    }

    fun exists(key: String): Either<KvError, Boolean> {
        check(!closed) { "Bucket already closed" }
        val (keyPtr, keyLen) = lowerStringToPtrLen(key)
        val retPtr = alloc(8, 4) // result<bool, error>: tag@0(1,1), payload@4(max(bool 1, error 4)=4)
        hostEventualExists(handle, keyPtr, keyLen, retPtr)
        return liftKvResult(retPtr) { loadByte(it).toInt() != 0 }
    }

    fun keys(): Either<KvError, List<String>> {
        check(!closed) { "Bucket already closed" }
        val retPtr = alloc(12, 4) // result<list<key>, error>: tag@0(1,1), payload@4(8,4)
        hostEventualBatchKeys(handle, retPtr)
        return liftKvResult(retPtr) { liftListOfString(it) }
    }

    fun getMany(keys: List<String>): Either<KvError, List<ByteArray?>> {
        check(!closed) { "Bucket already closed" }
        val (keysPtr, keysLen) = lowerListOfStringToPtrLen(keys)
        val retPtr = alloc(12, 4) // result<list<option<incoming-value>>, error>: tag@0(1,1), payload@4(8,4)
        hostEventualBatchGetMany(handle, keysPtr, keysLen, retPtr)
        return liftKvResult(retPtr) { listBase ->
            val dataPtr = loadInt(listBase)
            val len = loadInt(listBase + 4)
            (0 until len).map { i ->
                val elemPtr = dataPtr + i * 8 // option<incoming-value>: tag@0(1,1), payload@4(i32 handle,4,4)
                if (loadByte(elemPtr).toInt() == 0) null else consumeIncomingValue(loadInt(elemPtr + 4))
            }
        }
    }

    fun deleteMany(keys: List<String>): Either<KvError, Unit> {
        check(!closed) { "Bucket already closed" }
        val (keysPtr, keysLen) = lowerListOfStringToPtrLen(keys)
        val retPtr = alloc(8, 4)
        hostEventualBatchDeleteMany(handle, keysPtr, keysLen, retPtr)
        return liftKvUnitResult(retPtr)
    }

    fun close() {
        if (!closed) {
            hostBucketDrop(handle)
            closed = true
        }
    }

    companion object {
        /** Opens a bucket with the given name. */
        fun open(name: String): Either<KvError, Bucket> {
            val (namePtr, nameLen) = lowerStringToPtrLen(name)
            val retPtr = alloc(8, 4) // result<bucket, error>: tag@0(1,1), payload@4(4,4)
            hostOpenBucket(namePtr, nameLen, retPtr)
            return liftKvResult(retPtr) { Bucket(loadInt(it)) }
        }
    }
}

// Consumes and drops an incoming-value handle in one step -- this SDK doesn't expose
// incoming-value as public API (Scala's own Bucket.get/getMany do the same: construct an
// IncomingValue, consume it immediately, discard the wrapper).
private fun consumeIncomingValue(handle: Int): ByteArray {
    // 12 bytes, not 8: the payload is a (ptr, len) pair at offset 4. Under-allocating lets the
    // host's own cabi_realloc for the body bytes overlap the length slot, so the value reads back
    // as its own little-endian length (a 3-byte write came back as [3, 0, 0]).
    val retPtr = alloc(12, 4) // result<list<u8>, error>: tag@0(1,1), payload@4(8,4) -- errors here trap, see below
    hostIncomingValueConsumeSync(handle, retPtr)
    val result = liftKvResult(retPtr) { listBase ->
        val dataPtr = loadInt(listBase)
        val len = loadInt(listBase + 4)
        ByteArray(len) { i -> loadByte(dataPtr + i) }
    }
    hostIncomingValueDrop(handle)
    return when (result) {
        is Either.Right -> result.value
        // get/get-many's own result is already Either<KvError, ...>; consume-sync failing after
        // a successful get is an unexpected host-side inconsistency, not a normal error path --
        // mirrors this SDK's existing convention of surfacing unexpected failures as a message
        // (see HostApi.trap's callers) rather than inventing a third error channel here.
        is Either.Left -> error("incoming-value-consume-sync failed: ${result.value.message}")
    }
}
