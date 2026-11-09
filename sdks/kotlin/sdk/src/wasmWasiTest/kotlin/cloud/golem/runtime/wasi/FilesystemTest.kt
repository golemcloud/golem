@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime.wasi

import cloud.golem.wasm.alloc
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

/**
 * `liftFileSystemErrorCode` decodes `wasi:filesystem/types@0.3.0`'s `error-code` (size=16 align=4,
 * tag@0, payload@4), derived in this plan's Task 9 with `wit_parser::SizeAlign` against the real
 * WIT, not hand-computed. Same two-oracle style as `HttpTest.kt`'s `liftHttpErrorCode` tests: this
 * writes the raw bytes a real host would -- the independent oracle -- and checks the decoder
 * against them directly. Unlike `HttpErrorCode`, only one of this enum's 37 cases (`other`, tag 36)
 * carries a payload (`option<string>`), and this decoder discards it by design (a flat
 * `enum class`, per this file's own brief-specified shape), so `everyTagDecodesToItsOwnCase` checks
 * every tag against the enum directly rather than needing a separate payload-shape test.
 *
 * Task 11's live-host cases (large-file round-trip, a missing path decoding to `NoEntry` rather
 * than an exception) are not unit tests here -- `wasmWasiNodeTest` has no real `wasi:filesystem`
 * host to call, so `readBytes`/`writeBytes` cannot run in this file at all (same reason
 * `HttpOutgoingClient.send` itself has no unit test, only its pure `liftHttpErrorCode` does). They
 * live as `probeFilesystem`/`probeFilesystemLargeFile` in
 * `sdks/kotlin/scripts/contract-tests/ContractProbeAgent.kt`, run against a real `golem server
 * run` by `sdks/kotlin/scripts/native-contract-tests.sh` (probes 47/48) -- see this plan's Task 11
 * ledger entry for the actual round-trip results.
 */
class FilesystemTest {
    private fun errorCodeBuffer(): Int = alloc(16, 4)

    @Test
    fun aNoPayloadCaseDecodesByTagAlone() {
        val base = errorCodeBuffer()
        storeByte(base, 19) // no-entry
        assertEquals(FileSystemErrorCode.NoEntry, liftFileSystemErrorCode(base))
    }

    @Test
    fun theOtherCaseDecodesByTagAloneAndDiscardsItsPayload() {
        val base = errorCodeBuffer()
        storeByte(base, 36) // other
        storeByte(base + 4, 1) // option<string>: Some (irrelevant -- the enum has no payload to put it in)
        storeInt(base + 8, 0)
        storeInt(base + 12, 0)
        assertEquals(FileSystemErrorCode.Other, liftFileSystemErrorCode(base))
    }

    @Test
    fun everyTagDecodesToItsOwnCase() {
        val expectedByTag = listOf(
            FileSystemErrorCode.Access, FileSystemErrorCode.Already, FileSystemErrorCode.BadDescriptor,
            FileSystemErrorCode.Busy, FileSystemErrorCode.Deadlock, FileSystemErrorCode.Quota,
            FileSystemErrorCode.Exist, FileSystemErrorCode.FileTooLarge, FileSystemErrorCode.IllegalByteSequence,
            FileSystemErrorCode.InProgress, FileSystemErrorCode.Interrupted, FileSystemErrorCode.Invalid,
            FileSystemErrorCode.Io, FileSystemErrorCode.IsDirectory, FileSystemErrorCode.Loop,
            FileSystemErrorCode.TooManyLinks, FileSystemErrorCode.MessageSize, FileSystemErrorCode.NameTooLong,
            FileSystemErrorCode.NoDevice, FileSystemErrorCode.NoEntry, FileSystemErrorCode.NoLock,
            FileSystemErrorCode.InsufficientMemory, FileSystemErrorCode.InsufficientSpace, FileSystemErrorCode.NotDirectory,
            FileSystemErrorCode.NotEmpty, FileSystemErrorCode.NotRecoverable, FileSystemErrorCode.Unsupported,
            FileSystemErrorCode.NoTty, FileSystemErrorCode.NoSuchDevice, FileSystemErrorCode.Overflow,
            FileSystemErrorCode.NotPermitted, FileSystemErrorCode.Pipe, FileSystemErrorCode.ReadOnly,
            FileSystemErrorCode.InvalidSeek, FileSystemErrorCode.TextFileBusy, FileSystemErrorCode.CrossDevice,
            FileSystemErrorCode.Other,
        )
        assertEquals(37, expectedByTag.size, "every WIT error-code case must be listed exactly once above")
        for ((tag, expected) in expectedByTag.withIndex()) {
            val base = errorCodeBuffer()
            storeByte(base, tag.toByte())
            if (tag == 36) storeByte(base + 4, 0) // other: None, so this exercises its no-payload path too
            assertEquals(expected, liftFileSystemErrorCode(base), "tag $tag")
        }
    }

    @Test
    fun anOutOfRangeTagFailsLoudlyInsteadOfMisdecoding() {
        val base = errorCodeBuffer()
        storeByte(base, 37) // one past the last real case (0-36)
        assertFailsWith<IllegalStateException> { liftFileSystemErrorCode(base) }
    }

    @Test
    fun relativePathStripsTheLeadingSlash() {
        assertEquals("data/test.txt", relativePath("/data/test.txt"))
        assertEquals("", relativePath("/"))
    }

    @Test
    fun relativePathRejectsANonAbsolutePath() {
        assertFailsWith<IllegalArgumentException> { relativePath("data/test.txt") }
    }
}
