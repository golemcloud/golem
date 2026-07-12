@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.wasi

import cloud.golem.runtime.Either
import cloud.golem.wasm.alloc
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
class KvError internal constructor(handle: Int) {
    val message: String = run {
        val retPtr = alloc(8, 4)
        hostErrorTrace(handle, retPtr)
        val msg = liftString(loadInt(retPtr), loadInt(retPtr + 4))
        hostErrorDrop(handle)
        msg
    }
}

private fun <T> liftKvResult(base: Int, liftOk: (Int) -> T): Either<KvError, T> = if (loadByte(base).toInt() == 0) Either.Right(liftOk(base + 4)) else Either.Left(KvError(loadInt(base + 4)))

private fun liftKvUnitResult(base: Int): Either<KvError, Unit> = if (loadByte(base).toInt() == 0) Either.Right(Unit) else Either.Left(KvError(loadInt(base + 4)))

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
    val retPtr = alloc(8, 4) // result<list<u8>, error>: tag@0(1,1), payload@4(8,4) -- errors here trap, see below
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
