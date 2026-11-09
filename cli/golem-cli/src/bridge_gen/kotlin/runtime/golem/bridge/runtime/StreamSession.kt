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

import golem.bridge.runtime.json.Json
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.math.BigInteger
import java.net.URI
import java.net.http.HttpClient
import java.net.http.WebSocket
import java.net.http.WebSocketHandshakeException
import java.nio.ByteBuffer
import java.nio.charset.StandardCharsets
import java.util.Collections
import java.util.IdentityHashMap
import java.util.UUID
import java.util.WeakHashMap
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CompletionException
import java.util.concurrent.CompletionStage
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicReference

/**
 * One invocation of a stream-bearing agent method over one WebSocket, which multiplexes every input and output stream of that
 * invocation. Generated codecs reach the session through [input] and [output]: an input stream becomes a provisional reference in the
 * frozen start descriptor, and a server stream token becomes an [AgentStream] whose items the session delivers.
 */
object StreamSession {
    private val CANCEL_REASONS = setOf(
        "cancelled",
        "consumer-drop",
        "transport-detached",
        "source-unavailable",
        "producer-deleted",
        "invocation-failed",
        "protocol-error",
    )

    /** How long a live transport stays idle before the session pings it, and how long that ping has to come back. */
    private const val LIVENESS_PING_DELAY_MILLIS = 250L
    private const val LIVENESS_PING_TIMEOUT_MILLIS = 1000L

    /** How long a retryable connection failure waits before the next attempt. */
    private const val RECONNECT_DELAY_MILLIS = 50L

    /** The session a value is encoded into or decoded out of, bound for the duration of that one call. */
    private val current = ThreadLocal<Session>()

    /**
     * The session that owns each output stream of a delivered invocation result. A result is decoded by the caller, on its own thread
     * and after [invoke] has returned, so the stream reference the result carries is the only way back to the session. The key is held
     * weakly, so an entry lives exactly as long as the application's own reference to that stream value: the [output] call that
     * resolves it removes it, and a result the application drops undecoded retains nothing.
     */
    private val resultStreams: MutableMap<SchemaValue.StreamRef, Session> =
        Collections.synchronizedMap(WeakHashMap<SchemaValue.StreamRef, Session>())

    /** The output streams of delivered results that the application still holds and has not resolved. */
    internal val pendingResultStreams: Int get() = resultStreams.size

    private fun <A> scoped(session: Session, body: () -> A): A {
        val previous = current.get()
        current.set(session)
        return try {
            body()
        } finally {
            if (previous == null) current.remove() else current.set(previous)
        }
    }

    /**
     * Registers [stream] as an input of the invocation being started and returns the provisional reference that stands for it in the
     * start descriptor. [encode] renders one element, and [lane] is `u8` for a packed byte stream and `json` otherwise.
     */
    fun <T> input(stream: AgentStream<T>, encode: (T) -> SchemaValue, lane: String): SchemaValue {
        val session = current.get() ?: throw BridgeException("stream values may only be encoded while starting an invocation")
        val id = UUID.randomUUID().toString()
        val consumer = session.registerInput(stream)
        @Suppress("UNCHECKED_CAST")
        session.inputs[id] = Input(consumer, encode as (Any?) -> SchemaValue, lane, session.pendingInputBytes)
        return SchemaValue.StreamRef(id, null)
    }

    /** Resolves the server stream token [value] carries into the stream its items are delivered through. */
    fun <T> output(value: SchemaValue, decode: (SchemaValue) -> T, lane: String): AgentStream<T> {
        val reference = value as? SchemaValue.StreamRef ?: throw BridgeException("expected stream reference")
        if (reference.provisionalRef != null && reference.streamToken != null) {
            throw BridgeException("A stream reference must contain exactly one reference")
        }
        val token = reference.streamToken ?: throw BridgeException("server returned a provisional output stream")
        val session = current.get()?.takeIf { it.outputs.containsKey(token) }
            ?: resultStreams.remove(reference)
            ?: throw BridgeException("output stream is detached from its invocation")
        return session.output(token, decode, lane)
    }

    /** Every stream reference of [value], in the order the tree holds them. */
    internal fun streamReferences(value: SchemaValue): List<SchemaValue.StreamRef> {
        val found = ArrayList<SchemaValue.StreamRef>()
        collectStreams(value, found)
        return found
    }

    /**
     * Rejects a value tree that carries one stream reference at two structural coordinates. A stream is affine: it belongs to exactly
     * one place in one value, whichever direction the value travels.
     */
    internal fun requireDistinctStreams(value: SchemaValue) {
        val seen = HashSet<String>()
        for (reference in streamReferences(value)) {
            if (!seen.add(streamIdentity(reference))) throw BridgeException("stream reference appears more than once")
        }
    }

    /**
     * Offers one packed `u8` frame to [queue] as a chain of single-byte deliveries: each pull delivers the next byte and re-queues the
     * rest, and [delivered] runs once, after the last byte. Every delivery of the chain carries the frame's whole byte count, so the
     * frame holds exactly one charge against [charge] while any of its bytes are still queued: the queue releases that charge on the
     * pull that takes a delivery, and the delivery that re-queues the chain charges it again. [failed] sees a failure that reaches the
     * application through the pull.
     */
    internal fun offerPackedBytes(
        queue: StreamSessionState.OutputQueue<Any?>,
        payload: ByteArray,
        charge: AtomicLong,
        decode: (SchemaValue) -> Any?,
        delivered: () -> Unit,
        failed: (Throwable) -> Unit,
    ) {
        val bytes = payload.size
        fun delivery(index: Int): StreamSessionState.Delivery<Any?> = StreamSessionState.Delivery(
            bytes,
            {
                try {
                    val value = decode(SchemaValue.U8((payload[index].toInt() and 0xff).toUByte()))
                    if (index + 1 < bytes) {
                        charge.addAndGet(bytes.toLong())
                        if (!queue.continueAccepted(delivery(index + 1))) charge.addAndGet(-bytes.toLong())
                    } else {
                        delivered()
                    }
                    AgentStreamStep.Item(value)
                } catch (e: Throwable) {
                    failed(e)
                    throw e
                }
            },
            null,
        )
        queue.offer(delivery(0))
    }

    internal fun invoke(
        resolved: ResolvedAgent,
        method: String,
        parameters: () -> SchemaValue,
    ): CompletableFuture<AgentInvocationResult> = Invocation(resolved, method).start(parameters)

    private fun collectStreams(value: SchemaValue, found: MutableList<SchemaValue.StreamRef>) {
        when (value) {
            is SchemaValue.StreamRef -> found.add(value)
            is SchemaValue.Record -> value.fields.forEach { collectStreams(it, found) }
            is SchemaValue.ListVal -> value.items.forEach { collectStreams(it, found) }
            is SchemaValue.FixedListVal -> value.items.forEach { collectStreams(it, found) }
            is SchemaValue.TupleVal -> value.items.forEach { collectStreams(it, found) }
            is SchemaValue.OptionVal -> value.inner?.let { collectStreams(it, found) }
            is SchemaValue.ResultVal -> value.inner?.let { collectStreams(it, found) }
            is SchemaValue.VariantVal -> value.payload?.let { collectStreams(it, found) }
            is SchemaValue.MapVal -> value.entries.forEach { (key, item) ->
                collectStreams(key, found)
                collectStreams(item, found)
            }
            is SchemaValue.UnionVal -> collectStreams(value.body, found)
            else -> {}
        }
    }

    private fun streamIdentity(reference: SchemaValue.StreamRef): String = when {
        reference.provisionalRef != null && reference.streamToken == null -> "provisional:${reference.provisionalRef}"
        reference.provisionalRef == null && reference.streamToken != null -> "stable:${reference.streamToken}"
        else -> throw BridgeException("A stream reference must contain exactly one reference")
    }

    private fun packedByte(value: SchemaValue): Byte =
        (value as? SchemaValue.U8 ?: throw BridgeException("Expected a u8 value, got $value")).v.toByte()

    /**
     * The UTF-8 byte length of [data], counted rather than encoded, so each fragment of a text message adds to the byte count the
     * frozen message limit is measured in. A surrogate pair counts as the four bytes of its code point; a pair split across two
     * fragments counts three bytes in each, which charges six bytes for four and so rejects an oversized message sooner.
     */
    private fun utf8Length(data: CharSequence): Long {
        var length = 0L
        var index = 0
        while (index < data.length) {
            val character = data[index]
            length += when {
                character.code < 0x80 -> 1
                character.code < 0x800 -> 2
                Character.isHighSurrogate(character) && index + 1 < data.length && Character.isLowSurrogate(data[index + 1]) -> {
                    index++
                    4
                }
                else -> 3
            }
            index++
        }
        return length
    }

    /** A text message being reassembled: the characters received so far and the UTF-8 bytes they arrived as. */
    private class TextFrame {
        val text = StringBuilder()
        var bytes = 0L
    }

    private fun connectionCause(error: Throwable): Throwable {
        val cause = error.cause
        return if ((error is CompletionException || error is ExecutionException) && cause != null) connectionCause(cause) else error
    }

    /**
     * Whether another connection attempt can recover from [error]. A rejected handshake is the server's answer and repeating it
     * cannot change it, while a transport failure or a timeout describes the one attempt that failed.
     */
    private fun retryableConnectionFailure(error: Throwable): Boolean = when (connectionCause(error)) {
        is WebSocketHandshakeException -> false
        is IOException -> true
        is TimeoutException -> true
        else -> false
    }

    /** One registered input stream: its consumer, how one element renders, and the unacknowledged range it retains. */
    private class Input(
        val consumer: AgentStream.Consumer<Any?>,
        val encode: (Any?) -> SchemaValue,
        val lane: String,
        replayBytes: AtomicLong,
    ) {
        val state = StreamSessionState.InputState(replayBytes)

        @Volatile
        var bufferedPull: CompletableFuture<AgentStreamStep<Any?>>? = null

        @Volatile
        var naturalEnd = false

        @Volatile
        var cancelReason: String? = null

        val pulling = AtomicBoolean(false)
    }

    /** The streams of one invocation, by provisional reference and by stream token, and the two byte budgets they share. */
    private class Session {
        val inputs = ConcurrentHashMap<String, Input>()
        val inputTokens = ConcurrentHashMap<String, Input>()
        val outputs = ConcurrentHashMap<String, StreamSessionState.OutputQueue<Any?>>()
        val stableDirections = ConcurrentHashMap<String, String>()
        val cursors = ConcurrentHashMap<String, String>()
        val queuedBytes = AtomicLong(0L)
        val pendingInputBytes = AtomicLong(0L)
        private val inputIdentities = IdentityHashMap<AgentStream<*>, Boolean>()

        fun registerInput(stream: AgentStream<*>): AgentStream.Consumer<Any?> = synchronized(inputIdentities) {
            if (inputIdentities.put(stream, true) != null) {
                throw BridgeException("AgentStream was used at more than one structural coordinate")
            }
            try {
                @Suppress("UNCHECKED_CAST")
                AgentStream.claim(stream as AgentStream<Any?>)
            } catch (e: Throwable) {
                inputIdentities.remove(stream)
                throw e
            }
        }

        fun <T> output(token: String, decode: (SchemaValue) -> T, lane: String): AgentStream<T> {
            val queue = outputs[token] ?: throw BridgeException("output stream is detached from its invocation")
            @Suppress("UNCHECKED_CAST")
            queue.setDecoder(decode as (SchemaValue) -> Any?, lane)
            queue.expose()
            @Suppress("UNCHECKED_CAST")
            return AgentStream.remote(queue as AgentStream.Consumer<T>)
        }
    }

    private class Invocation(private val resolved: ResolvedAgent, private val method: String) {
        private val session = Session()
        private val finished = CompletableFuture<Void?>()
        private val channels = ConcurrentHashMap<Long, String>()
        private val directions = ConcurrentHashMap<Long, String>()
        private val tokenChannels = ConcurrentHashMap<String, Long>()
        private val idempotencyKey = UUID.randomUUID().toString()
        private val sendQueue = AtomicReference(CompletableFuture.completedFuture<Void?>(null))

        /** One client for every transport of this invocation, so a retry loop does not accumulate connection pools. */
        private val client by lazy { HttpClient.newHttpClient() }

        /**
         * Whether a reconnection is running, and whether one was asked for while it ran. A request that arrives during a reconnection
         * is remembered rather than dropped, and runs once the reconnection in progress has settled, so a transport that drops while
         * the session is reattaching still leaves the session reattaching.
         */
        private val reconnecting = AtomicBoolean(false)
        private val reconnectRequested = AtomicBoolean(false)

        /** The last liveness ping the session sent, and the last one the peer answered. */
        private val pingSequence = AtomicLong(0L)
        private val pongSequence = AtomicLong(0L)

        /**
         * The session's own result future. [start] hands back a copy of it, so a caller that cancels or completes what it holds
         * cannot settle this one and make the server's result look like a second one. The session drops this future as it settles it,
         * so a delivered result is then the caller's alone and the session retains neither the result nor the value tree whose stream
         * references key [resultStreams].
         */
        @Volatile
        private var pendingResult: CompletableFuture<AgentInvocationResult>? = CompletableFuture()

        @Volatile
        private var resultDelivered = false

        @Volatile
        private var socket: WebSocket? = null

        @Volatile
        private var accepted = false

        @Volatile
        private var sessionToken: String? = null

        @Volatile
        private var pendingDescriptor: String? = null

        @Volatile
        private var pendingAttempt: String? = null

        /** Whether the frozen descriptor attaches to an existing session, and which operation it attaches with. */
        @Volatile
        private var pendingResume = false

        @Volatile
        private var pendingAttachOperation = "resume"

        /** Whether the server has ever accepted an attempt of this invocation, which is what makes the session resumable. */
        @Volatile
        private var everAccepted = false

        fun start(parameters: () -> SchemaValue): CompletableFuture<AgentInvocationResult> {
            val caller = pendingResult!!.copy()
            val encoded = scoped(session) { parameters() }
            requireDistinctStreams(encoded)
            val selector = ArrayList<Pair<String, Json>>()
            selector.add("agentType" to Json.string(resolved.agentTypeName))
            selector.add("application" to Json.string(resolved.configuration.appName))
            selector.add("constructorParameters" to SchemaValueJson.toJson(resolved.parameters))
            selector.add("environment" to Json.string(resolved.configuration.envName))
            selector.add("method" to Json.string(method))
            resolved.phantomId?.let { selector.add("phantomId" to Json.string(it)) }
            val attempt = UUID.randomUUID().toString()
            pendingAttempt = attempt
            pendingDescriptor = StreamSessionProtocol.message(
                "invocationStart",
                listOf(
                    "attemptId" to Json.string(attempt),
                    "config" to Json.arr(emptyList()),
                    "idempotencyKey" to Json.string(idempotencyKey),
                    "methodParameters" to SchemaValueJson.toJson(encoded),
                    "selector" to Json.obj(selector),
                ),
            )
            reconnecting.set(true)
            connectRetry().whenComplete { _, error -> connectSettled(error) }
            return caller
        }

        /** Hands the result to the caller and releases the future. False once the result has been settled. */
        private fun settleResult(value: AgentInvocationResult): Boolean {
            val caller = pendingResult ?: return false
            pendingResult = null
            resultDelivered = true
            return caller.complete(value)
        }

        private fun failResult(error: Throwable) {
            val caller = pendingResult
            pendingResult = null
            caller?.completeExceptionally(error)
        }

        private fun connect(): CompletableFuture<Void?> = try {
            val base = resolved.configuration.server.url.trimEnd('/').replaceFirst(Regex("^http"), "ws")
            val builder = client.newWebSocketBuilder()
                .subprotocols(StreamSessionProtocol.SUBPROTOCOL)
                .header("Authorization", "Bearer ${resolved.configuration.server.token}")
            builder.buildAsync(URI.create("$base/v1/agents/${StreamSessionProtocol.ENDPOINT}"), listener)
                .thenCompose { ws ->
                    if (ws.subprotocol != StreamSessionProtocol.SUBPROTOCOL) {
                        CompletableFuture.failedFuture(BridgeException("server did not select golem.agent-invocation.v1"))
                    } else {
                        socket = ws
                        send(pendingDescriptor!!)
                    }
                }
        } catch (e: Throwable) {
            CompletableFuture.failedFuture(e)
        }

        /**
         * Connects, repeating an attempt that failed for a reason another attempt can recover from. An invocation that settles while
         * this runs leaves nothing to connect for, so the attempt is abandoned rather than reported as a new failure.
         */
        private fun connectRetry(): CompletableFuture<Void?> {
            val settled = CompletableFuture<Void?>()
            fun attempt() {
                if (finished.isDone) {
                    settled.complete(null)
                    return
                }
                connect().whenComplete { _, error ->
                    when {
                        error == null -> settled.complete(null)
                        retryableConnectionFailure(error) && !finished.isDone ->
                            CompletableFuture.delayedExecutor(RECONNECT_DELAY_MILLIS, TimeUnit.MILLISECONDS).execute { attempt() }
                        else -> settled.completeExceptionally(connectionCause(error))
                    }
                }
            }
            attempt()
            return settled
        }

        /** Releases the reconnection and runs the next one the session asked for while this one was connecting. */
        private fun connectSettled(error: Throwable?) {
            reconnecting.set(false)
            if (error != null) {
                fatal(connectionCause(error))
            } else if (reconnectRequested.get()) {
                reconnect()
            }
        }

        /**
         * Replaces the transport and reattaches the session to it. The channel table belongs to the transport that announced it, so it
         * is dropped; every input stream forgets its channel and every output queue rewinds to the checkpoint the application has
         * reached, and the frozen descriptor becomes a resume of the accepted session. An invocation that has already settled
         * reattaches to nothing: a frame still bound to the transport [fatal] aborted fails after that, and the session must let that
         * failure go rather than take over a session it has abandoned.
         */
        private fun reconnect() {
            if (finished.isDone) return
            reconnectRequested.set(true)
            if (!reconnecting.compareAndSet(false, true)) return
            reconnectRequested.set(false)
            val previous = socket
            socket = null
            previous?.abort()
            channels.clear()
            directions.clear()
            tokenChannels.clear()
            session.inputs.values.forEach { it.state.detach() }
            session.outputs.values.forEach { it.prepareResume() }
            try {
                if (accepted) prepareResumeDescriptor()
            } catch (e: Throwable) {
                reconnecting.set(false)
                fatal(e)
                return
            }
            connectRetry().whenComplete { _, error -> connectSettled(error) }
        }

        /**
         * Freezes a `resumeAttach` descriptor for [operation]. A descriptor that attaches to the session and has not been accepted
         * keeps its own cursor set, so an escalation to a takeover carries the same evidence the resume carried and differs from it
         * only in the operation and the attempt it names.
         */
        private fun prepareResumeDescriptor(operation: String = "resume") {
            val token = sessionToken ?: throw BridgeException("accepted session has no resume token")
            val cursors = if (pendingResume && !accepted) {
                Json.requireField(Json.parse(pendingDescriptor!!), "outputCursors")
            } else {
                Json.arr(session.cursors.values.sorted().map { Json.string(it) })
            }
            val attempt = UUID.randomUUID().toString()
            pendingAttempt = attempt
            pendingAttachOperation = operation
            pendingDescriptor = StreamSessionProtocol.message(
                "resumeAttach",
                listOf(
                    "attemptId" to Json.string(attempt),
                    "operation" to Json.string(operation),
                    "outputCursors" to cursors,
                    "sessionToken" to Json.string(token),
                ),
            )
            pendingResume = true
            accepted = false
        }

        /**
         * Pings [ws] once it has been idle for the ping delay and replaces it unless the answer arrives within the pong deadline. Each
         * answered ping schedules the next one, so one chain follows one transport and stops as soon as that transport is replaced.
         */
        private fun scheduleLivenessPing(ws: WebSocket) {
            CompletableFuture.delayedExecutor(LIVENESS_PING_DELAY_MILLIS, TimeUnit.MILLISECONDS).execute {
                if (ws !== socket || finished.isDone) return@execute
                val sequence = pingSequence.incrementAndGet()
                val payload = ByteBuffer.allocate(java.lang.Long.BYTES)
                payload.putLong(sequence)
                payload.flip()
                try {
                    ws.sendPing(payload).whenComplete { _, error ->
                        if (error != null) {
                            if (ws === socket) reconnect()
                        } else {
                            CompletableFuture.delayedExecutor(LIVENESS_PING_TIMEOUT_MILLIS, TimeUnit.MILLISECONDS).execute {
                                if (ws !== socket || finished.isDone) return@execute
                                if (pongSequence.get() < sequence) reconnect() else scheduleLivenessPing(ws)
                            }
                        }
                    }
                } catch (e: Throwable) {
                    if (ws === socket) reconnect()
                }
            }
        }

        /**
         * `java.net.http.WebSocket` rejects a text or binary send while another one is still pending, so every data frame goes through
         * one FIFO chain. Each send is bound to the socket that was current when it was enqueued: if that socket has been replaced by
         * the time the send runs, or fails on it, the frame is dropped and the send reports success, because the per-stream state
         * resends everything the server has not acknowledged once the channels are remapped onto the new socket.
         */
        private fun enqueueSend(operation: (WebSocket) -> CompletableFuture<WebSocket>): CompletableFuture<Void?> {
            val target = socket
            val completion = CompletableFuture<Void?>()
            val previous = sendQueue.getAndSet(completion)
            previous.whenComplete { _, _ ->
                if (target == null || target !== socket) {
                    completion.complete(null)
                } else {
                    try {
                        operation(target).whenComplete { _, error ->
                            if (error == null || target !== socket) {
                                completion.complete(null)
                            } else {
                                completion.completeExceptionally(error)
                            }
                        }
                    } catch (e: Throwable) {
                        completion.completeExceptionally(e)
                    }
                }
            }
            return completion
        }

        private fun send(text: String): CompletableFuture<Void?> = enqueueSend { it.sendText(text, true) }

        private fun sendBinary(metadata: Json, payload: ByteArray): CompletableFuture<Void?> {
            val frame = StreamSessionProtocol.binary(metadata, payload)
            return enqueueSend { it.sendBinary(frame, true) }
        }

        private fun cancel(channel: Long, reason: String): CompletableFuture<Void?> = send(
            StreamSessionProtocol.message(
                "streamCancel",
                listOf("channel" to Json.long(channel), "reason" to Json.string(reason)),
            ),
        )

        /** A frame the current transport failed to carry leaves the session attached to a transport it cannot use. */
        private fun CompletableFuture<Void?>.reconnectOnError(): CompletableFuture<Void?> =
            whenComplete { _, error -> if (error != null) reconnect() }

        private fun fatal(error: Throwable) {
            failResult(error)
            finished.completeExceptionally(error)
            session.outputs.values.forEach { if (!it.isTerminal) it.fail(error) }
            session.inputs.values.forEach { it.consumer.cancel() }
            socket?.abort()
        }

        private fun closeNormally() {
            socket?.sendClose(WebSocket.NORMAL_CLOSURE, "complete")
        }

        private fun highWater(mapping: Json): Pair<BigInteger, Boolean> {
            val value = Json.requireField(mapping, "inputHighWater")
            StreamSessionProtocol.validateObject(value, setOf("sequence", "terminal"), "inputHighWater")
            return StreamSessionProtocol.u64(value, "sequence") to Json.asBoolean(Json.requireField(value, "terminal"))
        }

        private class Mapping(
            val channel: Long,
            val token: String,
            val direction: String,
            val provisional: String?,
            val input: Input?,
            val inputHighWater: Pair<BigInteger, Boolean>?,
        )

        /**
         * Installs the channel table a message announces. Every mapping is checked against the table already installed, so a channel,
         * a stream token and a provisional reference each stay bound to one stream for the whole session. [complete] holds for an
         * acceptance: an initial one must map every input stream the client registered, and a resumed one must map every stream the
         * session already knows.
         */
        private fun install(json: Json, complete: Boolean = false) {
            val mappings = Json.asArray(Json.field(json, "mappings") ?: return)
            if (mappings.size > StreamSessionProtocol.MAX_STREAM_MAPPINGS) throw BridgeException("too many stream mappings")
            val messageChannels = HashSet<Long>()
            val messageTokens = HashSet<String>()
            val messageProvisionals = HashSet<String>()
            val parsed = mappings.map { mapping ->
                StreamSessionProtocol.validateMapping(mapping)
                val channel = StreamSessionProtocol.channel(mapping)
                val token = Json.asString(Json.requireField(mapping, "streamToken"))
                val direction = Json.asString(Json.requireField(mapping, "direction"))
                if (!messageChannels.add(channel) || !messageTokens.add(token)) throw BridgeException("duplicate stream mapping")
                val provisional = Json.field(mapping, "provisionalRef")?.let {
                    val reference = Json.asString(it)
                    if (!messageProvisionals.add(reference)) throw BridgeException("duplicate provisional stream mapping")
                    reference
                }
                StreamSessionProtocol.validateOpaqueToken(token, "stream token")
                if (direction != "input" && direction != "output") throw BridgeException("invalid stream direction")
                if (!session.stableDirections.containsKey(token) &&
                    session.stableDirections.size >= StreamSessionProtocol.MAX_STREAM_MAPPINGS
                ) {
                    throw BridgeException("too many stream mappings")
                }
                val stable = session.stableDirections[token]
                if (stable != null && stable != direction) throw BridgeException("stream token direction was rebound")
                val boundToken = channels[channel]
                if (boundToken != null && boundToken != token) throw BridgeException("stream channel was rebound")
                val boundDirection = directions[channel]
                if (boundDirection != null && boundDirection != direction) throw BridgeException("stream channel direction was rebound")
                val boundChannel = tokenChannels[token]
                if (boundChannel != null && boundChannel != channel) throw BridgeException("stream token received two channels")
                val provisionalInput = provisional?.let { session.inputs[it] }
                val knownInput = session.inputTokens[token]
                if (provisional != null && provisionalInput == null) {
                    throw BridgeException("input mapping contains an unknown provisional reference")
                }
                if (direction == "input" && provisionalInput == null && knownInput == null) {
                    throw BridgeException("input mapping does not identify a registered stream")
                }
                if (direction == "output" && provisional != null) throw BridgeException("provisional input mapped as output")
                val input = provisionalInput ?: knownInput
                if (input != null) {
                    if (knownInput != null && knownInput !== input) throw BridgeException("input stream token was rebound")
                    if (session.inputTokens.any { it.key != token && it.value === input }) {
                        throw BridgeException("input stream was rebound to a different token")
                    }
                }
                Mapping(channel, token, direction, provisional, input, if (direction == "input") highWater(mapping) else null)
            }
            if ((session.stableDirections.keys + messageTokens).size > StreamSessionProtocol.MAX_STREAM_MAPPINGS) {
                throw BridgeException("too many stream mappings")
            }
            if (complete) {
                if (pendingResume) {
                    if (!messageTokens.containsAll(session.stableDirections.keys)) {
                        throw BridgeException("resume acceptance omitted a known stream mapping")
                    }
                } else if (messageProvisionals != session.inputs.keys) {
                    throw BridgeException("initial acceptance omitted a provisional stream mapping")
                }
            }
            val pumps = LinkedHashSet<Input>()
            for (mapping in parsed) {
                session.stableDirections.putIfAbsent(mapping.token, mapping.direction)
                channels.putIfAbsent(mapping.channel, mapping.token)
                directions.putIfAbsent(mapping.channel, mapping.direction)
                tokenChannels.putIfAbsent(mapping.token, mapping.channel)
                if (mapping.direction == "output") {
                    session.outputs.computeIfAbsent(mapping.token) {
                        StreamSessionState.OutputQueue<Any?> { bytes -> session.queuedBytes.addAndGet(-bytes.toLong()) }
                    }
                }
                mapping.input?.let { input ->
                    val previous = session.inputTokens.putIfAbsent(mapping.token, input)
                    if (previous != null && previous !== input) throw BridgeException("input stream token was rebound")
                    val (sequence, terminal) = mapping.inputHighWater!!
                    val replay = input.state.remap(mapping.channel, sequence, terminal)
                    replay?.let { (pending, channel) -> pending.send(channel).reconnectOnError() }
                    val reason = input.cancelReason
                    if (terminal) {
                        input.cancelReason = null
                    } else if (reason != null) {
                        cancel(mapping.channel, reason).reconnectOnError()
                    } else if (replay == null) {
                        pumps.add(input)
                    }
                }
                session.outputs[mapping.token]
                    ?.remapCancel { reason -> cancel(mapping.channel, reason) }
                    ?.reconnectOnError()
            }
            pumps.forEach { pump(it) }
        }

        /** Takes one element from [input] and sends it, or sends the terminal the source reached. One pull is in flight at a time. */
        private fun pump(input: Input) {
            if (!input.pulling.compareAndSet(false, true)) return
            if (input.naturalEnd) {
                try {
                    sendTerminal(input)
                } catch (e: Throwable) {
                    fatal(e)
                }
                return
            }
            val pulled = try {
                val buffered = input.bufferedPull
                if (buffered != null) {
                    input.bufferedPull = null
                    buffered
                } else {
                    input.consumer.pull()
                }
            } catch (e: Throwable) {
                sourceUnavailable(input)
                return
            }
            pulled.whenComplete { step, error ->
                try {
                    if (error != null || step == null) {
                        sourceUnavailable(input)
                    } else if (step is AgentStreamStep.Item) {
                        sendItem(input, step.value)
                    } else {
                        sendTerminal(input)
                    }
                } catch (e: Throwable) {
                    fatal(e)
                }
            }
        }

        private fun sendTerminal(input: Input) {
            val first = input.state.nextSequence
            val pending = StreamSessionState.PendingTerminal(first) { channel ->
                send(
                    StreamSessionProtocol.message(
                        "inputStreamEnd",
                        listOf(
                            "channel" to Json.long(channel),
                            "sequence" to Json.string(StreamSessionProtocol.unsigned(first)),
                        ),
                    ),
                )
            }
            val (reserved, channel) = input.state.reserve(pending)
            input.pulling.set(false)
            if (channel > 0) reserved.send(channel).reconnectOnError()
        }

        private fun sendItem(input: Input, value: Any?) {
            try {
                val first = input.state.nextSequence
                val encoded = scoped(session) { input.encode(value) }
                val pending = if (input.lane == "u8") packedItems(input, first, encoded) else jsonItem(first, encoded)
                val (reserved, channel) = input.state.reserve(pending)
                input.pulling.set(false)
                if (channel > 0) reserved.send(channel).reconnectOnError()
            } catch (e: BridgeException) {
                throw e
            } catch (e: Throwable) {
                sourceUnavailable(input)
            }
        }

        private fun jsonItem(first: BigInteger, encoded: SchemaValue): StreamSessionState.PendingInput {
            val value = SchemaValueJson.toJson(encoded)
            val bytes = value.render().toByteArray(StandardCharsets.UTF_8).size
            return StreamSessionState.PendingData(first, BigInteger.ONE, bytes, { channel ->
                send(
                    StreamSessionProtocol.message(
                        "inputStreamItem",
                        listOf(
                            "channel" to Json.long(channel),
                            "sequence" to Json.string(StreamSessionProtocol.unsigned(first)),
                            "value" to value,
                        ),
                    ),
                )
            })
        }

        /**
         * Fills one packed `u8` frame from the elements the source already has ready. A pull that has not completed yet is retained for
         * the next item rather than waited for, so a slow source sends what it has instead of holding the frame open.
         */
        private fun packedItems(input: Input, first: BigInteger, encoded: SchemaValue): StreamSessionState.PendingInput {
            val bytes = ArrayList<Byte>()
            bytes.add(packedByte(encoded))
            var filling = true
            while (filling && bytes.size < StreamSessionProtocol.MAX_PACKED_BYTES) {
                val pulled = input.consumer.pull()
                if (!pulled.isDone) {
                    input.bufferedPull = pulled
                    filling = false
                } else {
                    val step = pulled.join()
                    if (step is AgentStreamStep.Item) {
                        bytes.add(packedByte(scoped(session) { input.encode(step.value) }))
                    } else {
                        input.naturalEnd = true
                        filling = false
                    }
                }
            }
            return packedFrame(first, bytes.toByteArray())
        }

        private fun packedFrame(start: BigInteger, payload: ByteArray): StreamSessionState.PendingInput =
            StreamSessionState.PendingData(
                start,
                BigInteger.valueOf(payload.size.toLong()),
                payload.size,
                { channel ->
                    sendBinary(
                        StreamSessionProtocol.inputBinaryMetadata("input-u8", channel, start, payload.size, null),
                        payload,
                    )
                },
                { highWater -> packedFrame(highWater, payload.copyOfRange(highWater.subtract(start).toInt(), payload.size)) },
            )

        private fun sourceUnavailable(input: Input) {
            input.pulling.set(false)
            input.cancelReason = "source-unavailable"
            input.state.cancelLocal()
            val channel = input.state.currentChannel
            if (channel > 0) cancel(channel, "source-unavailable").reconnectOnError()
        }

        private fun attemptId(json: Json, field: String): String {
            val text = Json.asString(Json.requireField(json, field))
            Uuid.fromStandardString(text)
            return text
        }

        private fun outputChannel(json: Json): Long {
            val channel = StreamSessionProtocol.channel(json)
            if (directions[channel] != "output") throw BridgeException("invalid output channel")
            return channel
        }

        private fun outputStream(channel: Long): Pair<String, StreamSessionState.OutputQueue<Any?>>? {
            val token = channels[channel] ?: return null
            val queue = session.outputs[token] ?: return null
            return token to queue
        }

        private fun readValue(raw: Json): SchemaValue {
            val value = scoped(session) { SchemaValueJson.fromJson(raw) }
            requireDistinctStreams(value)
            return value
        }

        private fun handle(json: Json) {
            val kind = Json.asString(Json.requireField(json, "type"))
            if (!accepted && kind != "invocationAccepted" && kind != "invocationRejected") {
                throw BridgeException("message received before invocation acceptance")
            }
            when (kind) {
                "invocationAccepted" ->
                    StreamSessionProtocol.validate(json, kind, setOf("attemptId", "idempotencyKey", "mappings", "sessionToken"))
                "invocationRejected" ->
                    StreamSessionProtocol.validate(json, kind, setOf("code", "message", "retryable"), setOf("attemptId"))
                "invocationResult" -> StreamSessionProtocol.validate(json, kind, setOf("mappings", "result"))
                "outputStreamItem" ->
                    StreamSessionProtocol.validate(json, kind, setOf("channel", "cursorToken", "mappings", "sequence", "value"))
                "outputStreamEnd" ->
                    StreamSessionProtocol.validate(json, kind, setOf("channel", "outcome", "sequence"), setOf("cursorToken"))
                "inputStreamAck" ->
                    StreamSessionProtocol.validate(json, kind, setOf("channel", "highestContiguousSequence", "mappings", "terminal"))
                "streamCancel" -> StreamSessionProtocol.validate(json, kind, setOf("channel", "reason"))
                "attachmentRevoked" -> StreamSessionProtocol.validate(json, kind, setOf("reason"))
                "invocationFinished" -> StreamSessionProtocol.validate(json, kind, setOf("outcome"))
                else -> throw BridgeException("unsupported stream session message: $kind")
            }
            when (kind) {
                "invocationAccepted" -> acceptInvocation(json)
                "invocationRejected" -> rejectInvocation(json)
                "invocationResult" -> deliverResult(json)
                "outputStreamItem" -> deliverOutputItem(json)
                "outputStreamEnd" -> deliverOutputTerminal(json)
                "inputStreamAck" -> acknowledgeInput(json)
                "streamCancel" -> cancelInput(json)
                "attachmentRevoked" -> revokeAttachment(json)
                "invocationFinished" -> finishInvocation(json)
            }
        }

        private fun acceptInvocation(json: Json) {
            if (accepted) throw BridgeException("duplicate invocation acceptance")
            val attempt = attemptId(json, "attemptId")
            val key = Json.asString(Json.requireField(json, "idempotencyKey"))
            if (attempt != pendingAttempt || key != idempotencyKey) {
                throw BridgeException("acceptance does not match the pending operation")
            }
            val token = Json.asString(Json.requireField(json, "sessionToken"))
            StreamSessionProtocol.validateOpaqueToken(token, "session token")
            install(json, complete = true)
            sessionToken = token
            accepted = true
            everAccepted = true
        }

        /**
         * A rejection of the attempt the session is waiting for. Once the session is resumable, a rejection the server marks as
         * retryable sends a new attach, and a resume the server rejects because the attachment it names is in an invalid state
         * escalates to a takeover. Every other rejection is the final answer for this invocation.
         */
        private fun rejectInvocation(json: Json) {
            val attempt = if (Json.field(json, "attemptId") != null) attemptId(json, "attemptId") else null
            if (attempt != null && attempt != pendingAttempt) throw BridgeException("rejection does not match the pending operation")
            if (attempt == null && everAccepted) throw BridgeException("rejection does not match the pending operation")
            val code = Json.asString(Json.requireField(json, "code"))
            val message = Json.asString(Json.requireField(json, "message"))
            val retryable = Json.asBoolean(Json.requireField(json, "retryable"))
            val pendingAttach = everAccepted && pendingResume && !accepted
            val takeover = pendingAttach && pendingAttachOperation == "resume" && code == "invalid-attachment-state"
            if (!takeover && !(everAccepted && retryable && code != "invalid-attachment-state")) {
                fatal(BridgeException("$code: $message"))
                return
            }
            try {
                prepareResumeDescriptor(
                    when {
                        takeover -> "takeover"
                        accepted -> "resume"
                        else -> pendingAttachOperation
                    },
                )
            } catch (e: Throwable) {
                fatal(e)
                return
            }
            reconnect()
        }

        private fun deliverResult(json: Json) {
            install(json)
            val envelope = Json.requireField(json, "result")
            val value = when (Json.asString(Json.requireField(envelope, "kind"))) {
                "none" -> {
                    StreamSessionProtocol.validateObject(envelope, setOf("kind"), "invocation result")
                    null
                }
                "value" -> {
                    StreamSessionProtocol.validateObject(envelope, setOf("graph", "kind", "value"), "invocation result")
                    val decoded = readValue(Json.requireField(envelope, "value"))
                    streamReferences(decoded).forEach { reference ->
                        reference.streamToken?.let { token -> if (session.outputs.containsKey(token)) resultStreams[reference] = session }
                    }
                    decoded
                }
                else -> throw BridgeException("invalid invocation result kind")
            }
            val agentId = resolved.agentId ?: AgentId("", "")
            if (!settleResult(AgentInvocationResult(agentId, idempotencyKey, value, null))) {
                throw BridgeException("duplicate invocation result")
            }
        }

        private fun deliverOutputItem(json: Json) {
            install(json)
            val channel = outputChannel(json)
            val sequence = StreamSessionProtocol.u64(json, "sequence")
            val cursor = Json.asString(Json.requireField(json, "cursorToken"))
            StreamSessionProtocol.validateOpaqueToken(cursor, "cursor token")
            val raw = Json.requireField(json, "value")
            val mapped = outputStream(channel) ?: return
            val (token, queue) = mapped
            queue.observeLane("json")
            queue.accept(sequence, BigInteger.ONE)
            val bytes = json.render().toByteArray(StandardCharsets.UTF_8).size
            admit(bytes)
            val next = StreamSessionProtocol.checkedEnd(sequence, BigInteger.ONE)
            queue.offer(
                StreamSessionState.Delivery(
                    bytes,
                    {
                        try {
                            val value = decodeItem(queue, readValue(raw))
                            queue.checkpoint(next)
                            session.cursors[token] = cursor
                            AgentStreamStep.Item(value)
                        } catch (e: Throwable) {
                            fatal(e)
                            throw e
                        }
                    },
                    null,
                ),
            )
        }

        private fun deliverOutputTerminal(json: Json) {
            val channel = outputChannel(json)
            val sequence = StreamSessionProtocol.u64(json, "sequence")
            val outcome = Json.requireField(json, "outcome")
            val terminal = Json.asString(Json.requireField(outcome, "kind"))
            when (terminal) {
                "ok" -> StreamSessionProtocol.validateObject(outcome, setOf("kind"), "output terminal")
                "error" -> StreamSessionProtocol.validateObject(outcome, setOf("kind", "code", "message"), "output terminal")
                "cancelled" -> {
                    StreamSessionProtocol.validateObject(outcome, setOf("kind", "reason"), "output terminal")
                    if (Json.asString(Json.requireField(outcome, "reason")) !in CANCEL_REASONS) {
                        throw BridgeException("invalid output cancellation reason")
                    }
                }
                else -> throw BridgeException("invalid output terminal outcome")
            }
            val cursor = Json.field(json, "cursorToken")?.let { Json.asString(it) }
            cursor?.let { StreamSessionProtocol.validateOpaqueToken(it, "cursor token") }
            val mapped = outputStream(channel) ?: return
            val (token, queue) = mapped
            queue.accept(sequence, BigInteger.ZERO)
            val delivered = {
                cursor?.let {
                    queue.checkpoint(sequence, terminal = true)
                    session.cursors[token] = it
                }
            }
            val delivery = when (terminal) {
                "ok" -> StreamSessionState.Delivery<Any?>(0, { delivered(); AgentStreamStep.End }, null)
                "error" -> StreamSessionState.Delivery<Any?>(0, null, {
                    delivered()
                    AgentStreamError(
                        Json.asString(Json.requireField(outcome, "code")),
                        Json.asString(Json.requireField(outcome, "message")),
                    )
                })
                else -> StreamSessionState.Delivery<Any?>(0, null, {
                    delivered()
                    AgentStreamCancelled(Json.asString(Json.requireField(outcome, "reason")))
                })
            }
            queue.offer(delivery, isTerminal = true)
        }

        private fun acknowledgeInput(json: Json) {
            install(json)
            val channel = StreamSessionProtocol.channel(json)
            if (directions[channel] != "input") throw BridgeException("invalid input ACK channel")
            val high = StreamSessionProtocol.u64(json, "highestContiguousSequence")
            val terminal = Json.asBoolean(Json.requireField(json, "terminal"))
            val input = channels[channel]?.let { session.inputTokens[it] }
                ?: throw BridgeException("unknown input ACK channel")
            if (input.state.acknowledge(high, terminal) && !terminal) pump(input)
        }

        private fun cancelInput(json: Json) {
            val channel = StreamSessionProtocol.channel(json)
            if (directions[channel] != "input") throw BridgeException("invalid input channel")
            if (Json.asString(Json.requireField(json, "reason")) !in CANCEL_REASONS) {
                throw BridgeException("invalid stream cancellation reason")
            }
            channels[channel]?.let { session.inputTokens[it] }?.let { input ->
                input.state.cancel()
                input.consumer.cancel()
            }
        }

        /** The server gave the session to another transport, so this one attaches again and becomes the owner in its turn. */
        private fun revokeAttachment(json: Json) {
            if (Json.asString(Json.requireField(json, "reason")) != "replaced") {
                throw BridgeException("invalid attachment revocation reason")
            }
            try {
                prepareResumeDescriptor()
            } catch (e: Throwable) {
                fatal(e)
                return
            }
            reconnect()
        }

        private fun finishInvocation(json: Json) {
            val outcome = Json.requireField(json, "outcome")
            when (Json.asString(Json.requireField(outcome, "kind"))) {
                "success" -> {
                    StreamSessionProtocol.validateObject(outcome, setOf("kind"), "invocation outcome")
                    if (!resultDelivered) throw BridgeException("invocation finished before its result")
                    if (session.outputs.values.any { !it.isTerminal }) {
                        throw BridgeException("invocation finished before an output terminal")
                    }
                    if (!finished.complete(null)) throw BridgeException("duplicate invocation finish")
                    closeNormally()
                }
                "failure" -> {
                    StreamSessionProtocol.validateObject(outcome, setOf("kind", "code", "message"), "invocation outcome")
                    val error = AgentStreamError(
                        Json.asString(Json.requireField(outcome, "code")),
                        Json.asString(Json.requireField(outcome, "message")),
                    )
                    if (session.outputs.values.any { !it.isTerminal }) {
                        throw BridgeException("invocation failure preceded an output terminal")
                    }
                    failResult(error)
                    if (!finished.complete(null)) throw BridgeException("duplicate invocation finish")
                    closeNormally()
                }
                else -> throw BridgeException("invalid invocation outcome")
            }
        }

        private fun decodeItem(queue: StreamSessionState.OutputQueue<Any?>, value: SchemaValue): Any? {
            val decode = queue.decode ?: throw BridgeException("output stream item arrived before the stream was exposed")
            return scoped(session) { decode(value) }
        }

        /** Takes [bytes] from the session's output budget, which the queue releases when the delivery holding them is pulled. */
        private fun admit(bytes: Int) {
            if (session.queuedBytes.addAndGet(bytes.toLong()) > StreamSessionProtocol.MAX_SESSION_QUEUED_BYTES) {
                session.queuedBytes.addAndGet(-bytes.toLong())
                throw BridgeException("session output queue exceeded the protocol limit")
            }
        }

        private val listener = object : WebSocket.Listener {
            private val texts = ConcurrentHashMap<WebSocket, TextFrame>()
            private val binaries = ConcurrentHashMap<WebSocket, ByteArrayOutputStream>()

            override fun onOpen(webSocket: WebSocket) {
                socket = webSocket
                webSocket.request(1)
                scheduleLivenessPing(webSocket)
            }

            override fun onText(webSocket: WebSocket, data: CharSequence, last: Boolean): CompletionStage<*>? {
                val frame = texts.computeIfAbsent(webSocket) { TextFrame() }
                frame.text.append(data)
                frame.bytes += utf8Length(data)
                if (frame.bytes > StreamSessionProtocol.MAX_MESSAGE_BYTES) {
                    throw BridgeException("text frame exceeds the protocol limit")
                }
                if (last) {
                    texts.remove(webSocket)
                    // `java.net.http.WebSocket` fails a text message whose bytes are not valid UTF-8 at the transport, so the
                    // characters that reach a listener are already a valid decoding of the frame.
                    if (webSocket === socket) scoped(session) { handle(Json.parse(frame.text.toString())) }
                }
                webSocket.request(1)
                return null
            }

            override fun onBinary(webSocket: WebSocket, data: ByteBuffer, last: Boolean): CompletionStage<*>? {
                val binary = binaries.computeIfAbsent(webSocket) { ByteArrayOutputStream() }
                val chunk = ByteArray(data.remaining())
                data.get(chunk)
                binary.write(chunk)
                if (binary.size() > StreamSessionProtocol.MAX_MESSAGE_BYTES) {
                    throw BridgeException("binary frame exceeds the protocol limit")
                }
                if (!last) {
                    webSocket.request(1)
                    return null
                }
                binaries.remove(webSocket)
                if (webSocket === socket) {
                    if (!accepted) throw BridgeException("binary message received before invocation acceptance")
                    scoped(session) { handleBinary(ByteBuffer.wrap(binary.toByteArray())) }
                }
                webSocket.request(1)
                return null
            }

            override fun onPong(webSocket: WebSocket, data: ByteBuffer): CompletionStage<*>? {
                if (webSocket === socket && data.remaining() == java.lang.Long.BYTES) pongSequence.set(data.getLong())
                webSocket.request(1)
                return null
            }

            /** A protocol violation ends the invocation; any other failure ends only this transport. */
            override fun onError(webSocket: WebSocket, error: Throwable) {
                if (webSocket !== socket) return
                if (error is BridgeException) fatal(error) else if (!finished.isDone) reconnect()
            }

            override fun onClose(webSocket: WebSocket, statusCode: Int, reason: String): CompletionStage<*>? {
                texts.remove(webSocket)
                binaries.remove(webSocket)
                if (webSocket === socket && !finished.isDone) reconnect()
                return null
            }
        }

        private fun handleBinary(frame: ByteBuffer) {
            val (metadata, payload) = StreamSessionProtocol.decodeBinary(frame)
            val kind = Json.asString(Json.requireField(metadata, "kind"))
            if (kind != "output-u8" && kind != "output-binary") throw BridgeException("invalid server binary lane")
            val channel = outputChannel(metadata)
            val sequence = StreamSessionProtocol.u64(metadata, "sequence")
            val count = StreamSessionProtocol.u64(metadata, "itemCount")
            StreamSessionProtocol.validateBinaryPayload(kind, payload, count)
            val cursor = Json.asString(Json.requireField(metadata, "cursorToken"))
            StreamSessionProtocol.validateOpaqueToken(cursor, "cursor token")
            val (token, queue) = outputStream(channel) ?: throw BridgeException("unknown output channel")
            queue.observeLane(if (kind == "output-u8") "u8" else "binary")
            StreamSessionProtocol.validateBinary(metadata, kind)
            val next = StreamSessionProtocol.checkedEnd(sequence, count)
            queue.accept(sequence, count)
            admit(payload.size)
            if (kind == "output-u8") {
                offerPackedBytes(
                    queue,
                    payload,
                    session.queuedBytes,
                    { value -> decodeItem(queue, value) },
                    {
                        queue.checkpoint(next)
                        session.cursors[token] = cursor
                    },
                    { error -> fatal(error) },
                )
            } else {
                val mime = Json.field(metadata, "mimeType")?.let { Json.asString(it) }
                val item = SchemaValue.BinaryVal(payload.map { (it.toInt() and 0xff).toUByte() }, mime)
                queue.offer(
                    StreamSessionState.Delivery(
                        payload.size,
                        {
                            try {
                                val value = decodeItem(queue, item)
                                queue.checkpoint(next)
                                session.cursors[token] = cursor
                                AgentStreamStep.Item(value)
                            } catch (e: Throwable) {
                                fatal(e)
                                throw e
                            }
                        },
                        null,
                    ),
                )
            }
        }
    }
}
