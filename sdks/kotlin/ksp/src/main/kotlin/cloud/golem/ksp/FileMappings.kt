package cloud.golem.ksp

import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction

/** A compiled public-path → filesystem mapping (`golem:agent/common`'s `file-mapping`). */
sealed class FileMapping {
    /** One public path served from one file. [publicPath] holds decoded segments (empty = the mount root). */
    data class Exact(val publicPath: List<String>, val filePath: String) : FileMapping()

    /** Every path below [publicPrefix] served from the matching file below [filesystemRoot]. */
    data class Subtree(val publicPrefix: List<String>, val filesystemRoot: String) : FileMapping()
}

/** Either a value or the corpus error code (`source-path`, `target-placeholder`, ...) explaining why it is invalid. */
sealed class Compiled<out T> {
    data class Ok<T>(val value: T) : Compiled<T>()
    data class Error(val code: String) : Compiled<Nothing>()
}

/**
 * Compiles `staticBindings` / `exposeFiles` declarations once, at build time, into structural mappings (a port of the Scala SDK's
 * `FileMappingParser`; both are checked against the shared corpus `golem-service-base/tests/fixtures/http-handlers/corpus.json`).
 * A source is an exact URI path or a path ending in a final slash-star wildcard; a subtree target ends in slash-dollar-one. Sources are percent-decoded exactly
 * once, after splitting into segments; targets are absolute filesystem text and are never URI-decoded.
 */
object FileMappings {
    fun compile(mappings: List<Pair<String, String>>): Compiled<List<FileMapping>> {
        val result = ArrayList<FileMapping>()
        for ((source, target) in mappings) {
            val mapping = when (val one = compileOne(source, target)) {
                is Compiled.Error -> return one
                is Compiled.Ok -> one.value
            }
            if (mapping in result) return Compiled.Error("duplicate-mapping")
            result.add(mapping)
        }
        return Compiled.Ok(result)
    }

    private fun compileOne(source: String, target: String): Compiled<FileMapping> {
        val subtree = source.endsWith("/*")
        val trimmed = if (subtree) source.dropLast(2) else source
        val path = if (subtree && trimmed.isEmpty()) "/" else trimmed
        if (subtree && trimmed.endsWith("/")) return Compiled.Error("source-path")
        if ('*' in path) return Compiled.Error("source-wildcard")
        val segments = when (val s = publicPath(path)) {
            is Compiled.Error -> return Compiled.Error("source-path")
            is Compiled.Ok -> s.value
        }
        val root: String
        if (subtree) {
            if (!target.endsWith("/$1") || '$' in target.dropLast(3)) return Compiled.Error("target-placeholder")
            if (target.dropLast(3).endsWith("/")) return Compiled.Error("target-path")
            root = target.dropLast(3).ifEmpty { "/" }
        } else {
            if ('$' in target) return Compiled.Error("target-placeholder")
            root = target
        }
        if (!(validFilesystemPath(root) && (subtree || root != "/"))) return Compiled.Error("target-path")
        return Compiled.Ok(if (subtree) FileMapping.Subtree(segments, root) else FileMapping.Exact(segments, root))
    }

    /** A public literal path split into segments, each decoded once (strict percent / UTF-8). */
    fun publicPath(path: String): Compiled<List<String>> {
        if (!path.startsWith("/") || path.any { it in "$*?#" }) return Compiled.Error("source-path")
        if (path == "/") return Compiled.Ok(emptyList())
        val segments = ArrayList<String>()
        for (raw in path.drop(1).split("/")) {
            when (val decoded = decodeSegment(raw)) {
                is Compiled.Error -> return decoded
                is Compiled.Ok -> segments.add(decoded.value)
            }
        }
        return Compiled.Ok(segments)
    }

    private fun decodeSegment(raw: String): Compiled<String> {
        val bytes = ArrayList<Byte>()
        var i = 0
        while (i < raw.length) {
            val c = raw[i]
            if (c == '%') {
                if (i + 2 >= raw.length) return Compiled.Error("source-path")
                val high = Character.digit(raw[i + 1], 16)
                val low = Character.digit(raw[i + 2], 16)
                if (high < 0 || low < 0 || raw[i + 1].code > 127 || raw[i + 2].code > 127) return Compiled.Error("source-path")
                bytes.add(((high shl 4) or low).toByte())
                i += 3
            } else {
                val plain = c.code <= 127 && (c.isLetterOrDigit() || c in "-._~!$&'()+,;=:@")
                if (!plain) return Compiled.Error("source-path")
                bytes.add(c.code.toByte())
                i++
            }
        }
        val decoded = try {
            Charsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT)
                .decode(ByteBuffer.wrap(bytes.toByteArray()))
                .toString()
        } catch (e: CharacterCodingException) {
            return Compiled.Error("source-path")
        }
        return if (validSegment(decoded)) Compiled.Ok(decoded) else Compiled.Error("source-path")
    }

    /** A non-empty path segment that is neither `.` nor `..` and contains no separator or control character. */
    fun validSegment(segment: String): Boolean = segment.isNotEmpty() &&
        segment != "." &&
        segment != ".." &&
        segment.none { it == '/' || it == '\\' || it < ' ' || it.code == 127 }

    fun validFilesystemPath(path: String): Boolean = path.none { Character.isISOControl(it) } &&
        (path == "/" || (path.startsWith("/") && '$' !in path && path.drop(1).split("/").all { validSegment(it) }))

    /** True if the compiled mappings are distinct and each is well-formed. */
    fun validateCompiled(mappings: List<FileMapping>): Boolean = mappings.distinct().size == mappings.size &&
        mappings.all {
            when (it) {
                is FileMapping.Exact -> it.publicPath.all { s -> validSegment(s) } && it.filePath != "/" && validFilesystemPath(it.filePath)
                is FileMapping.Subtree -> it.publicPrefix.all { s -> validSegment(s) } && validFilesystemPath(it.filesystemRoot)
            }
        }
}
