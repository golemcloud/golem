/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.bridge.runtime

import java.util.concurrent.CancellationException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CompletionException
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/** The most elements [AgentStream.toList] collects: the ceiling for a caller-chosen limit. */
const val ITEMS_MAX = 1_000_000

/** One step of a stream: an element, or the end. */
sealed class AgentStreamStep<out T> {
    data class Item<T>(val value: T) : AgentStreamStep<T>()

    object End : AgentStreamStep<Nothing>()
}

/** How a stream ended other than normally. */
sealed class AgentStreamTerminal(message: String) : RuntimeException(message)

/** The stream's source or the server reported a failure; [code] names it. */
class AgentStreamError(val code: String, val detail: String) : AgentStreamTerminal("$code: $detail")

/**
 * The stream was cancelled before it ended. [reason] carries either a protocol cancel reason or, when the cancellation originates
 * locally, a free-form local cause such as `interrupted`.
 */
class AgentStreamCancelled(val reason: String) : AgentStreamTerminal("stream cancelled: $reason")

/** The writing end handed to an [AgentStream.produce] body. */
class StreamSink<T> internal constructor(private val writer: (T) -> Boolean) {
    /**
     * Writes [value]; blocks until the reader takes it. Returns false once the reader has gone away, that is once the stream is closed or
     * cancelled. A body that ignores a false result keeps its pool thread until it returns.
     */
    fun write(value: T): Boolean = writer(value)
}

/** The daemon threads that run producer bodies, so a body never runs on, or keeps alive, the calling thread. */
internal object StreamExecutor {
    val executor: ExecutorService by lazy {
        Executors.newCachedThreadPool { task ->
            Thread(task, "golem-bridge-stream").apply { isDaemon = true }
        }
    }
}

/**
 * A demand-driven stream of [T]. Streams are affine: exactly one consumer reads a stream, either the caller through [next], [nextAsync]
 * and [toList], or the transport that carries it to or from the server. Pulling one item at a time is the back-pressure boundary.
 */
class AgentStream<T> private constructor(private val source: Consumer<T>) {
    /** The reading end of a stream. [cancel] is explicit; [drop] means an unread stream was abandoned. */
    interface Consumer<T> {
        fun pull(): CompletableFuture<AgentStreamStep<T>>

        fun cancel(): CompletableFuture<Void?>

        fun drop(): CompletableFuture<Void?>
    }

    private val claimed = AtomicBoolean(false)
    private val lock = Any()
    private var local: Consumer<T>? = null

    @Volatile
    private var closed = false

    private fun takeConsumer(): Consumer<T> {
        if (!claimed.compareAndSet(false, true)) {
            throw BridgeException(if (closed) "AgentStream is closed" else "AgentStream already has a consumer")
        }
        return source
    }

    private fun localConsumer(): Consumer<T> = synchronized(lock) {
        local ?: takeConsumer().also { local = it }
    }

    /** The next element, or null at the end of the stream. Blocks until one is available. */
    fun next(): T? = when (val step = await(nextAsync())) {
        is AgentStreamStep.Item -> step.value
        AgentStreamStep.End -> null
    }

    fun nextAsync(): CompletableFuture<AgentStreamStep<T>> = try {
        localConsumer().pull()
    } catch (e: BridgeException) {
        CompletableFuture.failedFuture(e)
    }

    /** Collects the rest of the stream, failing with a [BridgeException] if it holds more than [maxItems] elements. */
    fun toList(maxItems: Int = ITEMS_MAX): List<T> {
        if (maxItems < 0 || maxItems > ITEMS_MAX) throw BridgeException("maxItems must be between 0 and $ITEMS_MAX, got $maxItems")
        val out = ArrayList<T>()
        while (true) {
            val item = next() ?: return out
            if (out.size >= maxItems) {
                close()
                throw BridgeException("The stream holds more than $maxItems elements")
            }
            out.add(item)
        }
    }

    /** Releases the stream; a producer still writing sees its writes return false. Idempotent. */
    fun close() {
        val reader = synchronized(lock) {
            local ?: if (claimed.compareAndSet(false, true)) source.also { closed = true } else null
        }
        reader?.cancel()?.join()
    }

    companion object {
        /**
         * A stream whose elements [body] writes. The body runs on [StreamExecutor], never on the thread that reads the stream, and keeps
         * running until it returns, the stream is closed, or the reader is gone. A produced stream that is abandoned without [close] leaves
         * its body blocked in [StreamSink.write] on a pool thread; hand the stream to a consumer, drain it, or close it.
         */
        fun <T> produce(body: (StreamSink<T>) -> Unit): AgentStream<T> = AgentStream(ProducedConsumer(body))

        /** A stream of [values]. */
        fun <T> of(values: Iterable<T>): AgentStream<T> = produce { sink ->
            for (value in values) if (!sink.write(value)) break
        }

        internal fun <T> remote(consumer: Consumer<T>): AgentStream<T> = AgentStream(consumer)

        internal fun <T> claim(stream: AgentStream<T>): Consumer<T> = stream.takeConsumer()

        private fun <T> await(future: CompletableFuture<T>): T = try {
            future.join()
        } catch (e: CompletionException) {
            val cause = e.cause
            throw cause as? RuntimeException ?: BridgeException("Golem stream failed: ${cause?.message ?: e.message}", cause ?: e)
        } catch (e: CancellationException) {
            throw BridgeException("Golem stream was cancelled", e)
        }
    }

    /**
     * Hands elements from a producer body to a reader one at a time. The body starts on the first pull. A write returns only once the
     * reader has taken the element, or false once the stream is cancelled.
     */
    private class ProducedConsumer<T>(private val body: (StreamSink<T>) -> Unit) : Consumer<T> {
        private val lock = ReentrantLock()
        private val changed = lock.newCondition()
        private var started = false
        private var cancelled = false
        private var slot: AgentStreamStep.Item<T>? = null
        private var finished = false
        private var failure: Throwable? = null

        private fun write(value: T): Boolean = lock.withLock {
            try {
                while (slot != null && !cancelled) changed.await()
                if (cancelled) return false
                val item = AgentStreamStep.Item(value)
                slot = item
                changed.signalAll()
                while (slot === item && !cancelled) changed.await()
                slot !== item
            } catch (e: InterruptedException) {
                Thread.currentThread().interrupt()
                false
            }
        }

        private fun finish(error: Throwable?) = lock.withLock {
            finished = true
            failure = error
            changed.signalAll()
        }

        private fun start() {
            val begin = lock.withLock { (!started && !cancelled).also { started = true } }
            if (!begin) return
            StreamExecutor.executor.execute {
                try {
                    body(StreamSink(::write))
                    finish(null)
                } catch (e: Throwable) {
                    finish(e)
                }
            }
        }

        private fun take(): AgentStreamStep<T> = lock.withLock {
            try {
                while (slot == null && !finished && !cancelled) changed.await()
            } catch (e: InterruptedException) {
                Thread.currentThread().interrupt()
                throw AgentStreamCancelled("interrupted")
            }
            val item = slot
            if (cancelled) return@withLock AgentStreamStep.End
            if (item != null) {
                slot = null
                changed.signalAll()
                return item
            }
            val error = failure
            if (error != null && !cancelled) {
                failure = null
                throw if (error is AgentStreamTerminal) error else AgentStreamError("producer-error", error.message ?: error.toString())
            }
            AgentStreamStep.End
        }

        override fun pull(): CompletableFuture<AgentStreamStep<T>> {
            start()
            return CompletableFuture.supplyAsync({ take() }, StreamExecutor.executor)
        }

        override fun cancel(): CompletableFuture<Void?> {
            lock.withLock {
                cancelled = true
                changed.signalAll()
            }
            return CompletableFuture.completedFuture(null)
        }

        override fun drop(): CompletableFuture<Void?> = cancel()
    }
}
