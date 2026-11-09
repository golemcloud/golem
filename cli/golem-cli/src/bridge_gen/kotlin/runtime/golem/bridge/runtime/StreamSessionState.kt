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

import java.math.BigInteger
import java.util.concurrent.CompletableFuture
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

internal object StreamSessionState {
    interface PendingInput {
        val first: BigInteger
        val count: BigInteger
        val bytes: Int
        val terminal: Boolean
        val highWater: BigInteger get() = first.add(count)

        fun send(channel: Long): CompletableFuture<Void?>

        fun after(highWater: BigInteger): PendingInput
    }

    class PendingData(
        override val first: BigInteger,
        override val count: BigInteger,
        override val bytes: Int,
        private val sendFn: (Long) -> CompletableFuture<Void?>,
        private val trimFn: ((BigInteger) -> PendingInput)? = null,
    ) : PendingInput {
        override val terminal: Boolean get() = false

        override fun send(channel: Long): CompletableFuture<Void?> = sendFn(channel)

        override fun after(highWater: BigInteger): PendingInput {
            if (highWater == first) return this
            val trim = trimFn ?: throw BridgeException("input high-water split an atomic item")
            return trim(highWater)
        }
    }

    class PendingTerminal(
        override val first: BigInteger,
        private val sendFn: (Long) -> CompletableFuture<Void?>,
    ) : PendingInput {
        override val count: BigInteger get() = BigInteger.ZERO
        override val bytes: Int get() = 0
        override val terminal: Boolean get() = true

        override fun send(channel: Long): CompletableFuture<Void?> = sendFn(channel)

        override fun after(highWater: BigInteger): PendingInput = this
    }

    /** Durable client-side input state. Exactly one pending range is retained until its cumulative acknowledgement. */
    class InputState(private val totalReplayBytes: AtomicLong = AtomicLong(0L)) {
        private val lock = Any()
        private var channel = 0L
        private var next = BigInteger.ZERO
        private var ended = false
        private var cancelled = false
        private var pending: PendingInput? = null

        val currentChannel: Long get() = synchronized(lock) { channel }
        val nextSequence: BigInteger get() = synchronized(lock) { next }
        val isTerminal: Boolean get() = synchronized(lock) { ended }
        val pendingBytes: Int get() = synchronized(lock) { pending?.bytes ?: 0 }

        fun detach() = synchronized(lock) { channel = 0L }

        fun remap(newChannel: Long, highWater: BigInteger, terminal: Boolean): Pair<PendingInput, Long>? = synchronized(lock) {
            if (newChannel <= 0) throw BridgeException("invalid input channel")
            if (highWater.signum() < 0 || highWater > next) throw BridgeException("input high-water is beyond sent data")
            val current = pending
            if (current != null && (highWater < current.first || highWater > current.highWater)) {
                throw BridgeException("input high-water conflicts with pending range")
            }
            if (terminal && !ended) throw BridgeException("server reported an unsent input terminal")
            if (!terminal && ended && !cancelled && (current == null || !current.terminal)) {
                throw BridgeException("server lost the input terminal")
            }
            channel = newChannel
            if (current != null) {
                pending = if ((current.terminal && terminal) || (!current.terminal && highWater == current.highWater)) {
                    release(current)
                    null
                } else if (!current.terminal && highWater > current.first) {
                    val trimmed = current.after(highWater)
                    release(current)
                    charge(trimmed)
                    trimmed
                } else {
                    current
                }
            }
            pending?.let { it to channel }
        }

        fun reserve(data: PendingInput): Pair<PendingInput, Long> = synchronized(lock) {
            if (ended || pending != null || data.first != next || data.count.signum() < 0) {
                throw BridgeException("conflicting input stream state")
            }
            charge(data)
            pending = data
            next = next.add(data.count)
            if (data.terminal) ended = true
            data to channel
        }

        fun acknowledge(highWater: BigInteger, terminal: Boolean): Boolean = synchronized(lock) {
            if (highWater.signum() < 0 || highWater > next) throw BridgeException("input ACK is beyond sent data")
            val current = pending
            if (current == null) {
                if (highWater != next || terminal != ended) throw BridgeException("conflicting duplicate input ACK")
                return false
            }
            if (highWater < current.first || highWater > current.highWater) {
                throw BridgeException("input ACK conflicts with pending range")
            }
            if (terminal && !ended) throw BridgeException("input ACK reports an unsent terminal")
            val accepted = (current.terminal && terminal && highWater == next) ||
                (!current.terminal && highWater == current.highWater)
            if (accepted) {
                release(current)
                pending = null
            } else if (!current.terminal && highWater > current.first) {
                val trimmed = current.after(highWater)
                release(current)
                charge(trimmed)
                pending = trimmed
            }
            accepted
        }

        fun cancel() = synchronized(lock) {
            pending?.let { release(it) }
            pending = null
            ended = true
        }

        fun cancelLocal() = synchronized(lock) {
            cancel()
            cancelled = true
        }

        private fun charge(data: PendingInput) {
            val total = totalReplayBytes.addAndGet(data.bytes.toLong())
            if (total > StreamSessionProtocol.MAX_REPLAY_BYTES) {
                totalReplayBytes.addAndGet(-data.bytes.toLong())
                throw BridgeException("unacknowledged input exceeds the protocol limit")
            }
        }

        private fun release(data: PendingInput) {
            totalReplayBytes.addAndGet(-data.bytes.toLong())
        }
    }

    /**
     * One queued output item: the bytes it holds against the session budget, and exactly one of a thunk producing the step or a thunk
     * producing the failure. Both thunks are evaluated on the pull that reaches the item, so a throw surfaces there.
     */
    data class Delivery<T>(val bytes: Int, val step: (() -> AgentStreamStep<T>)?, val error: (() -> Throwable)?) {
        init {
            if ((step == null) == (error == null)) throw BridgeException("a delivery holds exactly one of a step and an error")
        }
    }

    /**
     * Bounded output queue that also tracks the received sequence, the checkpoint a resumed attachment rewinds to, and the lane.
     * The release callback is invoked exactly once for every delivery's bytes: when the delivery is pulled or handed to a waiting
     * pull, when it is discarded by cancellation, failure or resume, and when [offer] rejects it. [continueAccepted] does not release
     * a delivery it declines; the caller keeps that charge.
     */
    class OutputQueue<T>(private val release: (Int) -> Unit) : AgentStream.Consumer<T> {
        private val lock = Any()
        private val waiting = ArrayDeque<CompletableFuture<AgentStreamStep<T>>>()
        private val queued = ArrayDeque<Delivery<T>>()
        private var queuedBytes = 0L
        private var nextSeq = BigInteger.ZERO
        private var checkpointSequence = BigInteger.ZERO
        private var checkpointTerminal = false
        private var protocolTerminal = false
        private var consumerStopped = false
        private var pendingCancel: String? = null
        private var failure: Throwable? = null
        private val exposed = AtomicBoolean(false)
        private var expectedLane: String? = null
        private var observedLane: String? = null

        @Volatile
        var decode: ((SchemaValue) -> T)? = null
            private set

        @Volatile
        var cancelWith: (String) -> CompletableFuture<Void?> = { CompletableFuture.completedFuture(null) }

        val isTerminal: Boolean get() = synchronized(lock) { protocolTerminal }
        val nextSequence: BigInteger get() = synchronized(lock) { nextSeq }
        val queuedByteCount: Long get() = synchronized(lock) { queuedBytes }
        val queuedItemCount: Int get() = synchronized(lock) { queued.size }

        private fun evaluate(delivery: Delivery<T>): CompletableFuture<AgentStreamStep<T>> = try {
            val error = delivery.error
            if (error != null) CompletableFuture.failedFuture(error()) else CompletableFuture.completedFuture(delivery.step!!())
        } catch (e: Exception) {
            CompletableFuture.failedFuture(e)
        }

        private fun completeWaiter(waiter: CompletableFuture<AgentStreamStep<T>>, delivery: Delivery<T>) {
            try {
                val error = delivery.error
                if (error != null) waiter.completeExceptionally(error()) else waiter.complete(delivery.step!!())
            } catch (e: Exception) {
                waiter.completeExceptionally(e)
            }
        }

        /**
         * Runs [body] under the queue lock. Whatever [body] adds to the action list (releases, thunk evaluations, waiter completions)
         * runs after the lock is released, in order, even when [body] throws.
         */
        private inline fun <R> locked(body: (MutableList<() -> Unit>) -> R): R {
            val actions = ArrayList<() -> Unit>()
            try {
                return synchronized(lock) { body(actions) }
            } finally {
                for (action in actions) action()
            }
        }

        private fun discardQueued(actions: MutableList<() -> Unit>) {
            while (queued.isNotEmpty()) {
                val bytes = queued.removeFirst().bytes
                queuedBytes -= bytes
                actions.add { release(bytes) }
            }
        }

        private fun drainWaiters(actions: MutableList<() -> Unit>, finish: (CompletableFuture<AgentStreamStep<T>>) -> Unit) {
            while (waiting.isNotEmpty()) {
                val waiter = waiting.removeFirst()
                actions.add { finish(waiter) }
            }
        }

        override fun pull(): CompletableFuture<AgentStreamStep<T>> {
            var taken: Delivery<T>? = null
            val result = locked<CompletableFuture<AgentStreamStep<T>>?> { actions ->
                val head = queued.removeFirstOrNull()
                if (head != null) {
                    queuedBytes -= head.bytes
                    taken = head
                    actions.add { release(head.bytes) }
                    return@locked null
                }
                val error = failure
                if (error != null) {
                    failure = null
                    return@locked CompletableFuture.failedFuture<AgentStreamStep<T>>(error)
                }
                if (protocolTerminal || consumerStopped) return@locked CompletableFuture.completedFuture<AgentStreamStep<T>>(AgentStreamStep.End)
                val waiter = CompletableFuture<AgentStreamStep<T>>()
                waiting.addLast(waiter)
                waiter
            }
            return result ?: evaluate(taken!!)
        }

        private fun stop(reason: String): CompletableFuture<Void?> {
            val send = locked { actions ->
                discardQueued(actions)
                drainWaiters(actions) { it.complete(AgentStreamStep.End) }
                if (consumerStopped) return CompletableFuture.completedFuture(null)
                consumerStopped = true
                if (protocolTerminal) {
                    pendingCancel = null
                    return CompletableFuture.completedFuture(null)
                }
                pendingCancel = reason
                cancelWith
            }
            return sendCancel(send, reason)
        }

        private fun sendCancel(send: (String) -> CompletableFuture<Void?>, reason: String): CompletableFuture<Void?> = try {
            send(reason).handle { _, _ -> null }
        } catch (e: Exception) {
            CompletableFuture.completedFuture(null)
        }

        override fun cancel(): CompletableFuture<Void?> = stop("cancelled")

        override fun drop(): CompletableFuture<Void?> = stop("consumer-drop")

        fun fail(error: Throwable): Unit = locked { actions ->
            discardQueued(actions)
            if (waiting.isEmpty() && !consumerStopped) {
                failure = error
            } else {
                drainWaiters(actions) { it.completeExceptionally(error) }
            }
            protocolTerminal = true
        }

        fun offer(delivery: Delivery<T>, isTerminal: Boolean = false): Unit = locked { actions ->
            if (protocolTerminal) {
                actions.add { release(delivery.bytes) }
                throw BridgeException("message received after output stream terminal")
            }
            if (consumerStopped) {
                actions.add { release(delivery.bytes) }
                if (isTerminal) markTerminal()
                return
            }
            if (waiting.isEmpty()) {
                if (queued.size >= StreamSessionProtocol.MAX_QUEUED_ITEMS ||
                    queuedBytes + delivery.bytes > StreamSessionProtocol.MAX_QUEUED_BYTES
                ) {
                    actions.add { release(delivery.bytes) }
                    throw BridgeException("output stream queue exceeded the protocol limit")
                }
                queued.addLast(delivery)
                queuedBytes += delivery.bytes
            } else {
                val waiter = waiting.removeFirst()
                actions.add { release(delivery.bytes) }
                actions.add { completeWaiter(waiter, delivery) }
            }
            if (isTerminal) protocolTerminal = true
        }

        fun continueAccepted(delivery: Delivery<T>): Boolean = locked { actions ->
            if (consumerStopped) return false
            if (waiting.isEmpty()) {
                queued.addFirst(delivery)
                queuedBytes += delivery.bytes
            } else {
                val waiter = waiting.removeFirst()
                actions.add { release(delivery.bytes) }
                actions.add { completeWaiter(waiter, delivery) }
            }
            true
        }

        fun accept(first: BigInteger, count: BigInteger) = synchronized(lock) {
            if (protocolTerminal || first != nextSeq) throw BridgeException("output stream sequence conflict")
            nextSeq = StreamSessionProtocol.checkedEnd(first, count)
        }

        fun checkpoint(next: BigInteger, terminal: Boolean = false) = synchronized(lock) {
            if (next < checkpointSequence || next > nextSeq) throw BridgeException("output checkpoint sequence conflict")
            checkpointSequence = next
            if (terminal) checkpointTerminal = true
        }

        fun prepareResume(): Unit = locked { actions ->
            discardQueued(actions)
            nextSeq = checkpointSequence
            protocolTerminal = checkpointTerminal
        }

        fun markTerminal() = synchronized(lock) {
            protocolTerminal = true
            pendingCancel = null
        }

        fun remapCancel(send: (String) -> CompletableFuture<Void?>): CompletableFuture<Void?>? {
            val reason = synchronized(lock) {
                cancelWith = send
                pendingCancel
            }
            return reason?.let { sendCancel(send, it) }
        }

        fun observeLane(lane: String) = synchronized(lock) {
            if ((expectedLane != null && expectedLane != lane) || (observedLane != null && observedLane != lane)) {
                throw BridgeException("output stream lane changed")
            }
            observedLane = lane
        }

        fun setDecoder(decode: (SchemaValue) -> T, lane: String) = synchronized(lock) {
            if ((observedLane != null && observedLane != lane) || (expectedLane != null && expectedLane != lane)) {
                throw BridgeException("output stream lane changed")
            }
            expectedLane = lane
            this.decode = decode
        }

        fun expose() {
            if (!exposed.compareAndSet(false, true)) throw BridgeException("output stream was exposed more than once")
        }
    }
}
