package cloud.golem.streams

import cloud.golem.runtime.Cap
import cloud.golem.runtime.SchemaStreamReader
import cloud.golem.runtime.SchemaStreamSink
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.Tally

/** The most elements `toList` collects: the ceiling for a caller-chosen limit. */
internal const val ITEMS_MAX = 1_000_000

/** The writing end handed to an [AgentStream.produce] body. */
class StreamSink<T> internal constructor(private val sink: SchemaStreamSink, private val encode: (T) -> SchemaValue) {
    /** Writes [value]; blocks until the reader takes it. Returns false once the reader has gone away. */
    fun write(value: T): Boolean = sink.write(encode(value))
}

/**
 * A stream of [T] that travels as a `schema-value-stream`: an argument or result of an agent method, a tool, an HTTP body.
 * Streams are **affine**: sending one moves it, and a stream you are reading cannot also be forwarded.
 *
 * Reading (a received stream): [next] blocks for the next element and returns null at the end; [close] releases it.
 * Producing: [produce] takes a body that writes elements; the body runs **after** the invocation delivered its result (the host reads
 * the stream concurrently), so returning an [AgentStream] from a method makes its body keep running after the method returned.
 */
class AgentStream<T> private constructor(
    private val reader: SchemaStreamReader?,
    private val body: ((StreamSink<T>) -> Unit)?,
    private val elementWitType: String,
    private val decode: ((SchemaValue) -> T)?,
) {
    private var moved = false

    /** The next element, or null at the end of the stream. Only for a received stream. */
    fun next(): T? {
        check(!moved) { "this stream was moved" }
        val r = checkNotNull(reader) { "a produced stream is read by the receiver, not locally" }
        val value = r.next() ?: return null
        return checkNotNull(decode) { "a received stream has a decoder" }(value)
    }

    /**
     * Collects the rest of the stream (blocks until it ends).
     * @throws IllegalStateException if it holds more than [maxItems] elements.
     */
    fun toList(maxItems: Int = ITEMS_MAX): List<T> {
        Cap.requireLimit(maxItems, ITEMS_MAX, "maxItems")
        val tally = Tally(maxItems)
        val out = ArrayList<T>()
        while (true) {
            val item = next() ?: break
            check(tally.add(1)) { "the stream holds more than $maxItems elements" }
            out.add(item)
        }
        return out
    }

    /**
     * The same stream with [cleanup] attached: it runs exactly once when the stream ends, fails or is dropped. A produced stream keeps
     * producing after the result was delivered; a received one is read and re-written (so it can no longer be forwarded as a handle).
     */
    fun ensuring(cleanup: () -> Unit): AgentStream<T> {
        check(!moved) { "this stream was already moved" }
        moved = true
        val source = this
        return produce(elementWitType) { sink ->
            try {
                if (source.body != null) {
                    source.body.invoke(sink)
                } else {
                    val reader = checkNotNull(source.reader) { "a stream with no producer body is a received one" }
                    val decode = checkNotNull(source.decode) { "a received stream has a decoder" }
                    try {
                        while (true) {
                            val v = reader.next()?.let(decode) ?: break
                            if (!sink.write(v)) break
                        }
                    } finally {
                        reader.close()
                    }
                }
            } finally {
                cleanup()
            }
        }
    }

    /** Releases a received stream. Idempotent. */
    fun close() {
        reader?.close()
    }

    /**
     * Moves this stream into a schema value (an argument or result). A produced stream is queued to run after the result is delivered;
     * a received stream that was not read yet is forwarded as is. Used by generated code.
     */
    fun intoSchemaValue(encodeElement: (T) -> SchemaValue): SchemaValue {
        check(!moved) { "this stream was already moved" }
        moved = true
        if (reader != null) return SchemaValue.StreamVal(reader.takeHandleForForwarding())
        val produce = checkNotNull(body) { "a stream that is not received has a producer body" }
        return SchemaValue.OutStreamVal(elementWitType) { sink -> produce(StreamSink(sink, encodeElement)) }
    }

    companion object {
        /** A stream whose elements [body] writes. [elementWitType] is the element's WIT type (generated code supplies it). */
        fun <T> produce(elementWitType: String = "", body: (StreamSink<T>) -> Unit): AgentStream<T> = AgentStream(null, body, elementWitType, null)

        /** A stream produced from [values]. */
        fun <T> of(values: Iterable<T>): AgentStream<T> = produce { sink ->
            for (v in values) if (!sink.write(v)) break
        }

        /** Wraps a received stream handle. Used by generated code. */
        fun <T> fromHandle(handle: Int, elementWitType: String, decode: (SchemaValue) -> T): AgentStream<T> = AgentStream(SchemaStreamReader(handle, elementWitType), null, elementWitType, decode)
    }
}
