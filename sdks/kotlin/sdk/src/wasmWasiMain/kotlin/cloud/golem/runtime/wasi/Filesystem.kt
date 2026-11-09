@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime.wasi

import cloud.golem.runtime.Either
import cloud.golem.wasm.StallGuard
import cloud.golem.wasm.StreamStatus
import cloud.golem.wasm.alloc
import cloud.golem.wasm.awaitOperation
import cloud.golem.wasm.awaitSubtask
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt

// Raw canonical-ABI bindings to wasi:filesystem/preopens@0.3.0 and wasi:filesystem/types@0.3.0
// (WASI preview 3). Offsets and raw import names below were derived with `wit_parser::SizeAlign`
// plus MoonBit's own generated `ffi.mbt`/`top.mbt` (the ground-truth for a linked import's actual
// raw signature, independent of guest language -- confirmed necessary, not optional, by `open-at`'s
// own ABI below) against the real `wit-native/deps/filesystem` WIT, not hand-computed -- see this
// plan's Task 8/9 ledger entries.
//
// Resolution rule (Task 8's finding): the host unconditionally preopens the filesystem root at
// name `/` (and `.`, unused here) regardless of whether any `files:` entry is declared in
// `golem.yaml` -- IFS mounts share that same root, they do not get a separate preopen of their
// own. Every path this object accepts is therefore absolute (starts with `/`, matching IFS's own
// `targetPath` convention), resolved by opening it relative to the `/` preopen.

private const val ROOT_PREOPEN_NAME = "/"

/** `path-flags`: bit 0 is `symlink-follow`. Always followed, matching a plain POSIX `open()`. */
private const val SYMLINK_FOLLOW = 1

/** `descriptor-flags` bits used here: `read` = bit 0, `write` = bit 1. */
private const val DESCRIPTOR_FLAGS_READ = 1
private const val DESCRIPTOR_FLAGS_WRITE = 2

/** `open-flags` bits used here: `create` = bit 0, `truncate` = bit 3. */
private const val OPEN_FLAGS_CREATE_TRUNCATE = 1 or 8

private const val FILE_READ_CHUNK = 64 * 1024
private const val FILE_WRITE_CHUNK = 64 * 1024

private const val READ_VIA_STREAM = "[method]descriptor.read-via-stream"
private const val WRITE_VIA_STREAM = "[method]descriptor.write-via-stream"

// ---- preopens ----

@kotlin.wasm.WasmImport("wasi:filesystem/preopens@0.3.0", "get-directories")
private external fun hostGetDirectories(returnArea: Int)

// ---- descriptor ----

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", "[resource-drop]descriptor")
private external fun hostDescriptorDrop(handle: Int)

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", "[async-lower][method]descriptor.stat-at")
private external fun hostDescriptorStatAt(handle: Int, pathFlags: Int, pathPtr: Int, pathLen: Int, resultPtr: Int): Int

// `open-at` takes 6 logical arguments (handle, path-flags, path ptr/len, open-flags, descriptor-flags).
// Unlike `stat-at` (5 logical args, flattened directly into 5 raw i32 params), wit-bindgen's
// async-lower packs these into a caller-allocated 20-byte argument block instead of flattening them
// -- confirmed from MoonBit's generated `Descriptor::open_at`, not assumed from the WIT signature's
// arity (Task 9's ledger entry: flattening vs. packing is a per-function fact, not a rule derivable
// from argument count alone). Layout: handle:i32@0, path-flags:u8@4, path-ptr:i32@8, path-len:i32@12,
// open-flags:u8@16, descriptor-flags:u8@17.
@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", "[async-lower][method]descriptor.open-at")
private external fun hostDescriptorOpenAt(argsPtr: Int, resultPtr: Int): Int

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", READ_VIA_STREAM)
private external fun hostDescriptorReadViaStream(handle: Int, offset: Long, returnAreaPtr: Int)

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", "[async-lower][stream-read-0]$READ_VIA_STREAM")
private external fun hostReadViaStreamRead(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", "[stream-drop-readable-0]$READ_VIA_STREAM")
private external fun hostReadViaStreamDropReadable(stream: Int)

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", "[future-drop-readable-1]$READ_VIA_STREAM")
private external fun hostReadViaStreamFutureDropReadable(future: Int)

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", WRITE_VIA_STREAM)
private external fun hostDescriptorWriteViaStream(handle: Int, dataStream: Int, offset: Long): Int

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", "[stream-new-0]$WRITE_VIA_STREAM")
private external fun hostWriteViaStreamStreamNew(): Long

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", "[async-lower][stream-write-0]$WRITE_VIA_STREAM")
private external fun hostWriteViaStreamStreamWrite(stream: Int, ptr: Int, count: Int): Int

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", "[stream-drop-writable-0]$WRITE_VIA_STREAM")
private external fun hostWriteViaStreamStreamDropWritable(stream: Int)

@kotlin.wasm.WasmImport("wasi:filesystem/types@0.3.0", "[future-drop-readable-1]$WRITE_VIA_STREAM")
private external fun hostWriteViaStreamFutureDropReadable(future: Int)

/**
 * `wasi:filesystem/types@0.3.0`'s `error-code`, in WIT declaration order (confirmed by Task 9
 * against the real WIT). Flat -- only case 36 (`other`) carries a payload (`option<string>`), and
 * this enum discards that message, matching this file's own brief-specified `enum class` shape
 * (unlike `HttpErrorCode`, which is a sealed class because several of its cases carry payloads
 * callers need).
 */
enum class FileSystemErrorCode {
    Access,
    Already,
    BadDescriptor,
    Busy,
    Deadlock,
    Quota,
    Exist,
    FileTooLarge,
    IllegalByteSequence,
    InProgress,
    Interrupted,
    Invalid,
    Io,
    IsDirectory,
    Loop,
    TooManyLinks,
    MessageSize,
    NameTooLong,
    NoDevice,
    NoEntry,
    NoLock,
    InsufficientMemory,
    InsufficientSpace,
    NotDirectory,
    NotEmpty,
    NotRecoverable,
    Unsupported,
    NoTty,
    NoSuchDevice,
    Overflow,
    NotPermitted,
    Pipe,
    ReadOnly,
    InvalidSeek,
    TextFileBusy,
    CrossDevice,
    Other,
}

/** Lift `error-code`'s wire tag (the one byte at [ptr]) to its enum case. */
internal fun liftFileSystemErrorCode(ptr: Int): FileSystemErrorCode {
    val tag = loadByte(ptr).toInt() and 0xFF
    return FileSystemErrorCode.entries.getOrNull(tag) ?: error("liftFileSystemErrorCode: tag $tag out of range")
}

private fun lowerStringToPtrLen(s: String): Pair<Int, Int> {
    val bytes = s.encodeToByteArray()
    val ptr = alloc(bytes.size, 1)
    for (i in bytes.indices) storeByte(ptr + i, bytes[i])
    return ptr to bytes.size
}

/** Strips the leading `/` every path this object accepts must have (see the file header's resolution rule). */
internal fun relativePath(path: String): String {
    require(path.startsWith("/")) { "Filesystem path must be absolute: $path" }
    return path.substring(1)
}

/**
 * Runs [block] against the descriptor for the unconditionally-preopened `/` root (Task 8's
 * finding), dropping every descriptor `get-directories` returned -- including the ones this
 * doesn't use -- exactly once each.
 */
private fun <T> withRootDescriptor(block: (Int) -> T): T {
    val returnArea = alloc(8, 4)
    hostGetDirectories(returnArea)
    val arrayBase = loadInt(returnArea)
    val count = loadInt(returnArea + 4)
    var rootHandle = -1
    for (i in 0 until count) {
        val elementBase = arrayBase + i * 12
        val handle = loadInt(elementBase)
        val name = liftString(loadInt(elementBase + 4), loadInt(elementBase + 8))
        if (name == ROOT_PREOPEN_NAME) {
            rootHandle = handle
        } else {
            hostDescriptorDrop(handle)
        }
    }
    check(rootHandle >= 0) { "no \"$ROOT_PREOPEN_NAME\" preopened directory (Task 8 found this present against a real host)" }
    try {
        return block(rootHandle)
    } finally {
        hostDescriptorDrop(rootHandle)
    }
}

/** Opens [relPath] under [root], with [openFlags]/[descriptorFlags] -- the one `open-at` call [readBytes]/[writeBytes] share. */
private fun openUnder(root: Int, relPath: String, openFlags: Int, descriptorFlags: Int): Either<FileSystemErrorCode, Int> {
    val (pathPtr, pathLen) = lowerStringToPtrLen(relPath)
    val argsPtr = alloc(20, 4)
    storeInt(argsPtr, root)
    storeByte(argsPtr + 4, SYMLINK_FOLLOW.toByte())
    storeInt(argsPtr + 8, pathPtr)
    storeInt(argsPtr + 12, pathLen)
    storeByte(argsPtr + 16, openFlags.toByte())
    storeByte(argsPtr + 17, descriptorFlags.toByte())
    val resultPtr = alloc(20, 4)
    val status = hostDescriptorOpenAt(argsPtr, resultPtr)
    awaitSubtask(status)
    return if (loadByte(resultPtr).toInt() == 0) {
        Either.Right(loadInt(resultPtr + 4))
    } else {
        Either.Left(liftFileSystemErrorCode(resultPtr + 4))
    }
}

private fun readWholeFileStream(fileHandle: Int): ByteArray {
    val returnArea = alloc(8, 4)
    hostDescriptorReadViaStream(fileHandle, 0L, returnArea)
    val stream = loadInt(returnArea)
    val future = loadInt(returnArea + 4)
    val chunks = mutableListOf<ByteArray>()
    val buffer = alloc(FILE_READ_CHUNK, 1)
    while (true) {
        val status = hostReadViaStreamRead(stream, buffer, FILE_READ_CHUNK)
        val finalStatus = awaitOperation(stream, status)
        val code = StreamStatus.code(finalStatus)
        val count = StreamStatus.count(finalStatus)
        if (count > 0) {
            val chunk = ByteArray(count)
            for (i in 0 until count) chunk[i] = loadByte(buffer + i)
            chunks.add(chunk)
        }
        if (code != StreamStatus.COMPLETED) break
    }
    hostReadViaStreamDropReadable(stream)
    // v1 does not inspect read-via-stream's own completion future -- end of stream is read from the
    // stream's non-COMPLETED status, the same simplification Http.kt's readWholeBodyStream makes for
    // its own (identically-shaped) read future.
    hostReadViaStreamFutureDropReadable(future)
    val total = chunks.sumOf { it.size }
    val out = ByteArray(total)
    var offset = 0
    for (c in chunks) {
        c.copyInto(out, offset)
        offset += c.size
    }
    return out
}

/**
 * Pushes all of [data] through a fresh `stream<u8>` into [fileHandle] via `write-via-stream`.
 *
 * v1 does not read `write-via-stream`'s own completion future (dropped unread below) -- only the
 * stream-write status is checked. **Not yet verified against a real host**: Task 11 must confirm
 * whether a write failure always surfaces as a non-`COMPLETED` stream-write status (handled below
 * as [FileSystemErrorCode.Io], since the real cause lives only in the unread future) or can stay
 * silent until the future itself is read, and extend this function if so.
 */
private fun writeWholeFileStream(fileHandle: Int, data: ByteArray): Either<FileSystemErrorCode, Unit> {
    val packed = hostWriteViaStreamStreamNew()
    val readable = (packed and 0xFFFFFFFFL).toInt()
    val writable = (packed ushr 32).toInt()
    // The host takes ownership of the readable end and drains it concurrently (same ordering as
    // Blobstore.kt's proven outgoing-value-write-body pattern: hand the reader its end first).
    val future = hostDescriptorWriteViaStream(fileHandle, readable, 0L)
    val buffer = alloc(minOf(data.size, FILE_WRITE_CHUNK).coerceAtLeast(1), 1)
    var sent = 0
    var open = true
    val stall = StallGuard("filesystem write")
    try {
        while (open && sent < data.size) {
            val chunk = minOf(FILE_WRITE_CHUNK, data.size - sent)
            for (i in 0 until chunk) storeByte(buffer + i, data[sent + i])
            var done = 0
            while (done < chunk) {
                val status = awaitOperation(writable, hostWriteViaStreamStreamWrite(writable, buffer + done, chunk - done))
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
        hostWriteViaStreamStreamDropWritable(writable)
    }
    hostWriteViaStreamFutureDropReadable(future)
    return if (sent < data.size) Either.Left(FileSystemErrorCode.Io) else Either.Right(Unit)
}

/** File I/O under the host's unconditionally-preopened `/` root (see this file's header). See `golem-file-io-kotlin`. */
object Filesystem {
    /** The preopened directory names visible to this component (Task 8: always includes `/`). */
    fun preopenedDirectories(): List<String> {
        val returnArea = alloc(8, 4)
        hostGetDirectories(returnArea)
        val arrayBase = loadInt(returnArea)
        val count = loadInt(returnArea + 4)
        val names = mutableListOf<String>()
        for (i in 0 until count) {
            val elementBase = arrayBase + i * 12
            hostDescriptorDrop(loadInt(elementBase))
            names.add(liftString(loadInt(elementBase + 4), loadInt(elementBase + 8)))
        }
        return names
    }

    /** Reads the full contents of [path] (absolute, e.g. an IFS `targetPath`). */
    fun readBytes(path: String): Either<FileSystemErrorCode, ByteArray> = withRootDescriptor { root ->
        when (val opened = openUnder(root, relativePath(path), openFlags = 0, descriptorFlags = DESCRIPTOR_FLAGS_READ)) {
            is Either.Left -> opened
            is Either.Right -> {
                val fileHandle = opened.value
                try {
                    Either.Right(readWholeFileStream(fileHandle))
                } finally {
                    hostDescriptorDrop(fileHandle)
                }
            }
        }
    }

    /** [readBytes], decoded as UTF-8. */
    fun readText(path: String): Either<FileSystemErrorCode, String> = when (val bytes = readBytes(path)) {
        is Either.Left -> bytes
        is Either.Right -> Either.Right(bytes.value.decodeToString())
    }

    /** Creates or replaces [path] with [data] (`create` + `truncate`, matching `writeFileSync`). */
    fun writeBytes(path: String, data: ByteArray): Either<FileSystemErrorCode, Unit> = withRootDescriptor { root ->
        when (
            val opened = openUnder(
                root,
                relativePath(path),
                openFlags = OPEN_FLAGS_CREATE_TRUNCATE,
                descriptorFlags = DESCRIPTOR_FLAGS_WRITE,
            )
        ) {
            is Either.Left -> opened
            is Either.Right -> {
                val fileHandle = opened.value
                try {
                    writeWholeFileStream(fileHandle, data)
                } finally {
                    hostDescriptorDrop(fileHandle)
                }
            }
        }
    }

    /** Whether [path] exists, via `stat-at` (matching `existsSync`'s swallow-errors-as-false semantics). */
    fun exists(path: String): Boolean = withRootDescriptor { root ->
        val (pathPtr, pathLen) = lowerStringToPtrLen(relativePath(path))
        val resultPtr = alloc(112, 8)
        val status = hostDescriptorStatAt(root, SYMLINK_FOLLOW, pathPtr, pathLen, resultPtr)
        awaitSubtask(status)
        loadByte(resultPtr).toInt() == 0
    }
}
