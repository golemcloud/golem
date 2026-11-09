package p19

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.config.Config
import cloud.golem.config.Secret
import cloud.golem.streams.DurableStreamCheckpoint
import cloud.golem.streams.DurableStreamException
import cloud.golem.streams.DurableStreamProducer
import cloud.golem.streams.DurableStreamReadOptions
import cloud.golem.streams.DurableStreams

data class AuthConfig(val bearer: Secret<String>)

@Agent(mount = "/ds/{id}", description = "durable streams spike")
class DsAgent(val id: String) : BaseAgent() {
    @Endpoint(post = "/read-json")
    fun readJson(url: String, offset: String): List<String> =
        DurableStreams.json(url, DurableStreamReadOptions(checkpoint = DurableStreamCheckpoint(offset))).toList()

    @Endpoint(post = "/read-bytes")
    fun readBytes(url: String): String {
        val chunks = DurableStreams.bytes(url).toList()
        return chunks.flatMap { it.toList() }.joinToString(",") { (it.toInt() and 0xFF).toString() } + " chunks=${chunks.size}"
    }

    @Endpoint(post = "/append-json")
    fun appendJson(url: String): String {
        val w = DurableStreams.jsonWriter(url, DurableStreamProducer("kt-producer", 4))
        val a = w.append(listOf("1", "[2,3]"))
        val b = w.append(listOf("\"x\""), close = true)
        return "$a | $b | closed=${w.isClosed} pending=${w.hasPending}"
    }

    @Endpoint(post = "/append-bytes")
    fun appendBytes(url: String): String {
        val w = DurableStreams.byteWriter(url, "application/octet-stream", DurableStreamProducer("kt-bytes", 1))
        val a = w.append(byteArrayOf(0, 255.toByte(), 7))
        w.dispose()
        return "$a disposed=${w.isClosed}"
    }

    @Endpoint(post = "/gone")
    fun gone(url: String): String = try {
        DurableStreams.json(url, DurableStreamReadOptions(checkpoint = DurableStreamCheckpoint("bogus"))).toList().toString()
    } catch (e: DurableStreamException) {
        "kind=${e.kind}"
    }
}

@Agent(mount = "/ds-auth/{id}", description = "durable streams with a bearer secret")
class DsAuthAgent(val id: String, val config: Config<AuthConfig>) : BaseAgent() {
    @Endpoint(post = "/read")
    fun read(url: String): List<String> = DurableStreams.json(url, auth = config.value.bearer).toList()

    @Endpoint(post = "/append")
    fun append(url: String): String {
        val w = DurableStreams.jsonWriter(url, DurableStreamProducer("kt-auth", 0), auth = config.value.bearer)
        return w.append(listOf("true"), close = true).toString()
    }
}
