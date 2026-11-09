package cloud.golem.gradle

import java.io.File
import java.io.IOException
import java.net.URI
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.MessageDigest

/**
 * Resolves the WASI preview1->preview2 adapter (`wasi_snapshot_preview1.reactor.wasm`) that
 * `wasm-tools component new` needs for Kotlin/Wasm's WASI p1 imports.
 *
 * The adapter is downloaded from the official wasmtime release on first use, verified against a
 * pinned SHA-256 and cached, so every build uses the same known-good bytes regardless of what
 * happens to be installed locally. The version tracks the wasmtime Golem's executor is built
 * against (`wasmtime = "=46.0.1"` in the workspace `Cargo.toml`).
 *
 * Do not fall back to an adapter found on disk: a `wit-bindgen` checkout's copy traps in
 * `random_get` (hit by `Any.hashCode()`, `HashMap` of plain objects, `Random`).
 */
internal object WasiAdapter {
    const val VERSION = "46.0.1"
    const val SHA256 = "0acb10959bd3c1d2e7903ef82212910bfc156ab5698ef3f2ff669474ba59fb0a"
    const val FILE_NAME = "wasi_snapshot_preview1.reactor.wasm"
    const val DEFAULT_URL =
        "https://github.com/bytecodealliance/wasmtime/releases/download/v$VERSION/$FILE_NAME"

    /** `$GRADLE_USER_HOME` (or `~/.gradle`) `/caches/golem/wasi-adapter/<version>`. */
    fun defaultCacheDir(): File {
        val gradleHome = System.getenv("GRADLE_USER_HOME")?.takeIf { it.isNotBlank() }
            ?: File(System.getProperty("user.home"), ".gradle").path
        return File(gradleHome, "caches/golem/wasi-adapter/$VERSION")
    }

    /**
     * Returns the adapter to use: [explicit] when given (must exist), otherwise the verified cached
     * download, fetching it from [url] into [cacheDir] first if needed.
     */
    fun resolve(
        explicit: String?,
        cacheDir: File = defaultCacheDir(),
        url: String = DEFAULT_URL,
        expectedSha256: String = SHA256,
    ): File {
        if (!explicit.isNullOrBlank()) {
            val f = File(explicit)
            require(f.isFile) { "wasmComponent.wasiAdapterPath does not exist: $f" }
            return f
        }
        val cached = File(cacheDir, FILE_NAME)
        if (cached.isFile && sha256(cached) == expectedSha256) return cached
        cacheDir.mkdirs()
        val tmp = File.createTempFile("adapter-", ".part", cacheDir)
        try {
            try {
                // GitHub rejects Java's default request headers with HTTP 406.
                val conn = URI(url).toURL().openConnection().apply {
                    setRequestProperty("User-Agent", "golem-kotlin-gradle-plugin")
                    setRequestProperty("Accept", "application/octet-stream, */*")
                    connectTimeout = 30_000
                    readTimeout = 60_000
                }
                conn.getInputStream().use { input -> Files.copy(input, tmp.toPath(), StandardCopyOption.REPLACE_EXISTING) }
            } catch (e: IOException) {
                throw IllegalStateException(
                    "Could not download the WASI p1->p2 adapter from $url (${e.message}). Download it manually and set " +
                        "wasmComponent.wasiAdapterPath, e.g. the wasmtime v$VERSION release asset $FILE_NAME.",
                    e,
                )
            }
            val actual = sha256(tmp)
            check(actual == expectedSha256) {
                "WASI adapter downloaded from $url has SHA-256 $actual, expected $expectedSha256 -- refusing to use it"
            }
            Files.move(tmp.toPath(), cached.toPath(), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
        } finally {
            tmp.delete()
        }
        return cached
    }

    private fun sha256(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buf = ByteArray(8192)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                digest.update(buf, 0, n)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }
}
