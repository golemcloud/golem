package p30

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Languages
import cloud.golem.annotations.MimeTypes
import cloud.golem.annotations.RemoteAgent
import cloud.golem.runtime.SchemaValue
import cloud.golem.schema.UnstructuredBinary
import cloud.golem.schema.UnstructuredText

@RemoteAgent(typeName = "Restricted")
interface RestrictedClient {
    fun text(t: @Languages("en", "de") UnstructuredText): String
    fun bin(b: @MimeTypes("image/png") UnstructuredBinary): String
}

@Agent(mount = "/restricted/{id}", description = "allow-listed unstructured types")
class Restricted(val id: String) : BaseAgent() {
    @Endpoint(post = "/text")
    fun text(t: @Languages("en", "de") UnstructuredText): String = t.toString()

    @Endpoint(post = "/bin")
    fun bin(b: @MimeTypes("image/png") UnstructuredBinary): String = b.toString()
}

@Agent(mount = "/restricted-caller/{id}", description = "calls Restricted")
class RestrictedCaller(val id: String) : BaseAgent() {
    @Endpoint(post = "/run")
    fun run(): String {
        val c = RestrictedClientRpc(SchemaValue.Record(listOf(SchemaValue.Str("r1"))))
        fun attempt(label: String, f: () -> String) = label + "=" + try { f() } catch (e: Throwable) { "ERR(" + (e.message ?: e.toString()).take(160) + ")" }
        try {
            return listOf(
                attempt("text-de") { c.text(UnstructuredText.fromInline("Hallo", "de")) },
                attempt("text-fr") { c.text(UnstructuredText.fromInline("Bonjour", "fr")) },
                attempt("text-url") { c.text(UnstructuredText.fromUrl("https://x/y.txt")) },
                attempt("bin-png") { c.bin(UnstructuredBinary.fromInline(byteArrayOf(1), "image/png")) },
                attempt("bin-gif") { c.bin(UnstructuredBinary.fromInline(byteArrayOf(1), "image/gif")) },
                attempt("text-none") { c.text(UnstructuredText.fromInline("Hi")) },
            ).joinToString(" | ")
        } finally {
            c.close()
        }
    }
}
