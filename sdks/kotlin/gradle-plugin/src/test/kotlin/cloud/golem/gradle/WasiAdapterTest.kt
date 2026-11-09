package cloud.golem.gradle

import java.io.File
import java.nio.file.Files
import java.security.MessageDigest
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class WasiAdapterTest {
    private fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    private fun source(bytes: ByteArray): String {
        val dir = Files.createTempDirectory("adapter-src").toFile()
        return File(dir, WasiAdapter.FILE_NAME).also { it.writeBytes(bytes) }.toURI().toString()
    }

    private fun cache() = Files.createTempDirectory("adapter-cache").toFile()

    @Test
    fun `explicit path is used as is`() {
        val f = File.createTempFile("custom", ".wasm").also { it.writeBytes(byteArrayOf(1)) }
        assertEquals(f, WasiAdapter.resolve(f.path, cacheDir = cache()))
    }

    @Test
    fun `explicit path must exist`() {
        val ex = assertFailsWith<IllegalArgumentException> { WasiAdapter.resolve("/no/such/adapter.wasm", cacheDir = cache()) }
        assertTrue(ex.message!!.contains("wasiAdapterPath"), ex.message)
    }

    @Test
    fun `downloads, verifies and caches the adapter`() {
        val bytes = "adapter-bytes".toByteArray()
        val cache = cache()
        val got = WasiAdapter.resolve(null, cache, source(bytes), sha(bytes))
        assertEquals(File(cache, WasiAdapter.FILE_NAME), got)
        assertContentEquals(bytes, got.readBytes())
        assertEquals(listOf(WasiAdapter.FILE_NAME), cache.list()!!.toList(), "no leftover temp files")
    }

    @Test
    fun `reuses a valid cached adapter without downloading`() {
        val bytes = "cached".toByteArray()
        val cache = cache()
        File(cache, WasiAdapter.FILE_NAME).writeBytes(bytes)
        // The URL does not exist: resolving must not touch it.
        val got = WasiAdapter.resolve(null, cache, "file:///does/not/exist", sha(bytes))
        assertContentEquals(bytes, got.readBytes())
    }

    @Test
    fun `replaces a corrupt cached adapter`() {
        val good = "good".toByteArray()
        val cache = cache()
        File(cache, WasiAdapter.FILE_NAME).writeBytes("corrupt".toByteArray())
        val got = WasiAdapter.resolve(null, cache, source(good), sha(good))
        assertContentEquals(good, got.readBytes())
    }

    @Test
    fun `rejects a download with the wrong hash and caches nothing`() {
        val cache = cache()
        val ex = assertFailsWith<IllegalStateException> {
            WasiAdapter.resolve(null, cache, source("tampered".toByteArray()), sha("expected".toByteArray()))
        }
        assertTrue(ex.message!!.contains("refusing to use it"), ex.message)
        assertFalse(File(cache, WasiAdapter.FILE_NAME).exists())
        assertTrue(cache.list()!!.isEmpty(), "no leftover temp files")
    }

    @Test
    fun `download failure points at wasiAdapterPath`() {
        val ex = assertFailsWith<IllegalStateException> {
            WasiAdapter.resolve(null, cache(), "file:///does/not/exist", "0")
        }
        assertTrue(ex.message!!.contains("wasmComponent.wasiAdapterPath"), ex.message)
    }
}
