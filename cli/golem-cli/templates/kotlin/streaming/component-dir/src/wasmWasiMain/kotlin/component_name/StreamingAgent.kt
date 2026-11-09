package component_name

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.DurableStreamOptions
import cloud.golem.annotations.DurableStreamSlot
import cloud.golem.annotations.Endpoint
import cloud.golem.config.Config
import cloud.golem.config.Secret
import cloud.golem.runtime.Either
import cloud.golem.streams.AgentStream
import cloud.golem.streams.DurableStreamProducer
import cloud.golem.streams.DurableStreams

/** The agent's configuration. `externalAuth` is an opaque secret: it is passed to the Durable Streams calls and never read or logged. */
data class StreamingConfig(val externalAuth: Secret<String>)

/**
 * Streams in three flavours:
 *  - **agent streams** (`AgentStream<T>`) as parameters and results of RPC methods: a stream is a lazy, back-pressured sequence handed to the
 *    peer, which reads it as it is produced;
 *  - **an HTTP route over Durable Streams** (`durableEcho`): the `input` and `$result` slots are exposed under `/streams/...` and can be written
 *    and read over HTTP, even by several readers;
 *  - **external Durable Streams** (`appendExternal` / `readExternal`): a durable client for any Durable Streams server.
 *
 * Mounted at /durable-stream-agents/{name}.
 */
@Agent(mount = "/durable-stream-agents/{name}", description = "Streams, Durable Streams routes and an external Durable Streams client")
class StreamingAgent(val name: String, val config: Config<StreamingConfig>) : BaseAgent() {
    private var cancelledProducers = 0

    /** Produces [values] one by one; counts the producers that were cancelled before they finished. */
    private fun <T> stream(values: List<T>): AgentStream<T> = AgentStream.produce { sink ->
        var emitted = 0
        for (value in values) {
            if (!sink.write(value)) {
                cancelledProducers += 1
                return@produce
            }
            emitted += 1
        }
    }

    @Endpoint
    fun sum(input: AgentStream<Int>): Int {
        var total = 0
        while (true) total += input.next() ?: break
        input.close()
        return total
    }

    @Endpoint
    fun produce(): AgentStream<Int> = stream(listOf(1, 2, 3))

    @Endpoint
    fun transform(prefix: String, input: AgentStream<Int>): AgentStream<String> = AgentStream.produce { sink ->
        while (true) {
            val value = input.next() ?: break
            if (!sink.write("$prefix:$value")) break
        }
        input.close()
    }

    /** A stream whose elements may fail individually: `Left` is an item that could not be produced, `Right` a value. */
    @Endpoint
    fun recoverable(): AgentStream<Either<String, Int>> = stream(listOf(Either.Right(1), Either.Left("this item could not be produced"), Either.Right(2)))

    @Endpoint
    fun status(): String = "ready ($cancelledProducers cancelled producers)"

    /**
     * Exposed as `PUT /durable-stream-agents/{name}/echo`: the `input` stream parameter is a writable Durable Stream and the result is a
     * readable one (slot `$result`), so an HTTP client can feed and follow the stream.
     */
    @Endpoint(put = "/echo")
    @DurableStreamSlot(source = "input", slot = "input")
    @DurableStreamSlot(source = "output", slot = "\$result")
    @DurableStreamOptions(allowExternalWrites = true)
    fun durableEcho(input: AgentStream<String>): AgentStream<String> = AgentStream.produce { sink ->
        while (true) {
            val value = input.next() ?: break
            if (!sink.write("echo:$value")) break
        }
        input.close()
    }

    /**
     * Appends [values] (plain strings, sent as JSON strings) to the JSON stream at [url] as [producerId]. Re-sending the same producer id and
     * sequence is idempotent: a duplicate returns `null` instead of writing again.
     */
    @Endpoint
    fun appendExternal(url: String, producerId: String, values: List<String>, close: Boolean): String? {
        val writer = DurableStreams.jsonWriter(url, DurableStreamProducer(producerId), auth = config.value.externalAuth)
        try {
            return writer.append(values.map(::jsonString), close).nextOffset
        } finally {
            writer.dispose()
        }
    }

    /** Reads the whole JSON stream at [url]; each element is the JSON text of one message. */
    @Endpoint
    fun readExternal(url: String): List<String> = DurableStreams.json(url, auth = config.value.externalAuth).toList()

    private fun jsonString(value: String): String = buildString {
        append('"')
        for (c in value) {
            when (c) {
                '"' -> append("\\\"")
                '\\' -> append("\\\\")
                '\n' -> append("\\n")
                '\r' -> append("\\r")
                '\t' -> append("\\t")
                else -> append(c)
            }
        }
        append('"')
    }
}
