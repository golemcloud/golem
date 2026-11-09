package p23

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.RemoteAgent
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.Uuid
import cloud.golem.schema.UnstructuredBinary
import cloud.golem.schema.UnstructuredText
import kotlin.time.Duration
import kotlin.time.Duration.Companion.milliseconds

data class Mixed(val id: Uuid, val wait: Duration, val note: UnstructuredText?, val data: ByteArray)

@RemoteAgent(typeName = "Corners")
interface CornersClient {
    fun uuid(u: Uuid): Uuid
    fun dur(d: Duration): Duration
    fun bytes(b: ByteArray): ByteArray
    fun text(t: UnstructuredText): UnstructuredText
    fun bin(b: UnstructuredBinary): UnstructuredBinary
    fun optionalText(t: UnstructuredText?): UnstructuredText?
}

@Agent(mount = "/corners/{id}", description = "scalar corners")
class Corners(val id: String) : BaseAgent() {
    @Endpoint(post = "/uuid")
    fun uuid(u: Uuid): Uuid = u

    @Endpoint(post = "/dur")
    fun dur(d: Duration): Duration = d + 1.milliseconds

    @Endpoint(post = "/bytes")
    fun bytes(b: ByteArray): ByteArray = b.reversedArray()

    @Endpoint(post = "/text")
    fun text(t: UnstructuredText): UnstructuredText = t

    @Endpoint(post = "/bin")
    fun bin(b: UnstructuredBinary): UnstructuredBinary = b

    @Endpoint(post = "/take-token")
    fun takeToken(t: cloud.golem.runtime.host.QuotaToken): Boolean = true

    @Endpoint(post = "/optional-text")
    fun optionalText(t: UnstructuredText?): UnstructuredText? = t
}

@Agent(mount = "/corners-caller/{id}", description = "calls Corners")
class CornersCaller(val id: String) : BaseAgent() {
    @Endpoint(post = "/run")
    fun run(): String {
        val c = CornersClientRpc(SchemaValue.Record(listOf(SchemaValue.Str("c1"))))
        try {
            val u = Uuid(-5L, 0x1122334455667788L)
            val out = mutableListOf<String>()
            out += "uuid=" + (c.uuid(u) == u)
            out += "dur=" + c.dur(1500.milliseconds)
            out += "bytes=" + c.bytes(byteArrayOf(1, 2, 3, -1)).toList()
            out += "text=" + c.text(UnstructuredText.fromInline("Hallo", "de")) + "," + c.text(UnstructuredText.fromUrl("https://x/y.txt"))
            out += "bin=" + c.bin(UnstructuredBinary.fromInline(byteArrayOf(9, 8), "image/png")) + "," + c.bin(UnstructuredBinary.fromUrl("https://x/y.png"))
            out += "opt=" + c.optionalText(null) + "," + c.optionalText(UnstructuredText.fromInline("hi"))
            return out.joinToString(" | ")
        } finally {
            c.close()
        }
    }
}
