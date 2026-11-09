package cloud.golem.ksp

import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/**
 * Runs the SDK-checkable cases of the shared HTTP corpus (`golem-service-base/tests/fixtures/http-handlers/corpus.json`, also executed by
 * the Scala SDK's `HttpCorpusSpec`): the `mapping-*` cases against [FileMappings] and the `metadata-*` cases against [RouterValidation].
 * The other suites (routes, envelopes, files, OpenAPI, lifecycle) describe host behaviour.
 */
class HttpCorpusTest {
    private val corpusFile = File("../../../golem-service-base/tests/fixtures/http-handlers/corpus.json")

    private fun cases(suite: String): List<Map<String, Any?>> {
        assertTrue(corpusFile.exists(), "the shared HTTP corpus is missing: ${corpusFile.absolutePath}")
        @Suppress("UNCHECKED_CAST")
        val all = (Json(corpusFile.readText()).value() as Map<String, Any?>)["cases"] as List<Map<String, Any?>>
        return all.filter { it["suite"] == suite }
    }

    @Suppress("UNCHECKED_CAST")
    private fun Any?.obj() = this as Map<String, Any?>

    @Suppress("UNCHECKED_CAST")
    private fun Any?.list() = this as List<Any?>

    private fun Any?.pairs(): List<Pair<String, String>> = this?.list().orEmpty().map { it.list().let { p -> p[0] as String to p[1] as String } }

    @Test
    fun mapping_cases() {
        val mappingCases = cases("mapping")
        assertEquals(11, mappingCases.size, "the corpus mapping suite changed; update this test")
        for (case in mappingCases) {
            val id = case["id"] as String
            val expect = case["expect"].obj()
            val result = FileMappings.compile(case["input"].obj()["mappings"].pairs())
            val error = expect["error"] as String?
            if (error != null) {
                assertEquals(Compiled.Error(error), result, id)
            } else {
                val expected = expect["compiled"].list().map { entry ->
                    val (kind, body) = entry.obj().entries.single()
                    val fields = body.obj()
                    if (kind == "Exact") {
                        FileMapping.Exact(fields["public_path"].list().map { it as String }, fields["file_path"] as String)
                    } else {
                        FileMapping.Subtree(fields["public_prefix"].list().map { it as String }, fields["filesystem_root"] as String)
                    }
                }
                assertEquals(Compiled.Ok(expected), result, id)
            }
        }
    }

    private fun segments(mount: String): List<MountSegment> = mount.split("/").filter { it.isNotEmpty() }.map {
        if (it.startsWith("{") && it.endsWith("}")) MountSegment.Variable(it.substring(1, it.length - 1)) else MountSegment.Literal(it)
    }

    private fun spec(input: Map<String, Any?>): AgentSpec {
        val aliases = input["schema_aliases"]?.obj().orEmpty()
        val overrides = input["schema_overrides"]?.obj().orEmpty().keys

        // A nominal name only counts as the canonical HTTP type through an alias; an overridden definition no longer matches it.
        fun type(t: String): String = if (t in overrides) "$t (overridden)" else (aliases[t] as String? ?: t)
        val methods = input["methods"]?.list().orEmpty().map { m ->
            val method = m.obj()
            MethodSpec(
                method["name"] as String,
                method["input"].pairs().map { it.first to type(it.second) },
                type(method["output"] as String),
                method["bindings"].pairs(),
                method.keys.any { it.startsWith("endpoint_") },
            )
        }
        return AgentSpec(
            kind = input["kind"] as String,
            mode = input["mode"] as String,
            phantom = input["phantom"] as Boolean? ?: false,
            snapshotEnabled = input["snapshot"] as Boolean? ?: false,
            constructor = input["constructor"].pairs(),
            mount = segments(input["mounts"]?.list()?.single() as String? ?: ""),
            staticBindings = input["static_bindings"].pairs(),
            filesystemBindings = input["filesystem_bindings"].pairs(),
            methods = methods,
            provider = input["provider"] as String?,
        )
    }

    @Test
    fun metadata_cases() {
        val metadataCases = cases("metadata")
        assertEquals(16, metadataCases.size, "the corpus metadata suite changed; update this test")
        for (case in metadataCases) {
            val id = case["id"] as String
            val expect = case["expect"].obj()
            val result = RouterValidation.validate(spec(case["input"].obj()))
            val error = expect["error"] as String?
            if (error != null) assertEquals(error, result, id) else assertEquals(null, result, id)
        }
    }

    /** A just-enough JSON reader for the corpus (objects, arrays, strings, numbers, booleans, null). */
    private class Json(private val text: String) {
        private var i = 0

        fun value(): Any? {
            skip()
            return when (val c = text[i]) {
                '{' -> obj()
                '[' -> arr()
                '"' -> str()
                't' -> literal("true", true)
                'f' -> literal("false", false)
                'n' -> literal("null", null)
                else -> if (c == '-' || c.isDigit()) number() else error("unexpected '$c' at $i")
            }
        }

        private fun skip() {
            while (i < text.length && text[i].isWhitespace()) i++
        }

        private fun literal(word: String, v: Any?): Any? {
            check(text.startsWith(word, i))
            i += word.length
            return v
        }

        private fun number(): Any {
            val start = i
            while (i < text.length && (text[i].isDigit() || text[i] in "-+.eE")) i++
            return text.substring(start, i).toDouble()
        }

        private fun str(): String {
            val sb = StringBuilder()
            i++
            while (text[i] != '"') {
                if (text[i] == '\\') {
                    i++
                    when (val e = text[i]) {
                        'n' -> sb.append('\n')
                        't' -> sb.append('\t')
                        'r' -> sb.append('\r')
                        'b' -> sb.append('\b')
                        'f' -> sb.append('\u000c')
                        'u' -> {
                            sb.append(text.substring(i + 1, i + 5).toInt(16).toChar())
                            i += 4
                        }
                        else -> sb.append(e)
                    }
                } else {
                    sb.append(text[i])
                }
                i++
            }
            i++
            return sb.toString()
        }

        private fun arr(): List<Any?> {
            val out = ArrayList<Any?>()
            i++
            skip()
            if (text[i] == ']') {
                i++
                return out
            }
            while (true) {
                out.add(value())
                skip()
                if (text[i++] == ']') return out
            }
        }

        private fun obj(): Map<String, Any?> {
            val out = LinkedHashMap<String, Any?>()
            i++
            skip()
            if (text[i] == '}') {
                i++
                return out
            }
            while (true) {
                skip()
                val key = str()
                skip()
                check(text[i++] == ':')
                out[key] = value()
                skip()
                if (text[i++] == '}') return out
            }
        }
    }
}
