/* Copyright 2024-2026 Golem Cloud. Licensed under the Golem Source License v1.1. */
package golem.bridge.runtime

import golem.bridge.runtime.AgentStreamStep.{End, Item}
import golem.bridge.runtime.SchemaValue.StreamReferenceValue
import golem.bridge.runtime.json.Json
import java.net.URI
import java.net.http.{HttpClient, WebSocket, WebSocketHandshakeException}
import java.io.{ByteArrayOutputStream, IOException}
import java.nio.ByteBuffer
import java.util.UUID
import java.util.concurrent.{CompletableFuture, CompletionException, CompletionStage, ConcurrentHashMap, ExecutionException, TimeUnit}
import java.util.concurrent.atomic.{AtomicBoolean, AtomicLong, AtomicReference}
import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.jdk.CollectionConverters._
import scala.util.{Failure, Success}

private[runtime] trait StreamBinding {
  def output[A](token: String, decode: SchemaValue => A, lane: String, codec: PublicValueCodec.Codec): AgentStream[A]
}

/** Internal bridge between generated recursive codecs and one WebSocket session. */
object StreamSession {
  private val LivenessPingDelayMillis = 250L
  private val LivenessPingTimeoutMillis = 1000L

  private final class Input(
    val consumer: Future[AgentStream.Consumer[Any]],
    val encode: Any => SchemaValue,
    val lane: String,
    val codec: PublicValueCodec.Codec,
    replayBytes: AtomicLong
  ) {
    val state = new StreamSessionState.InputState(replayBytes)
    @volatile var bufferedPull: Future[AgentStreamStep[Any]] = null
    @volatile var naturalEnd = false
    @volatile var cancelReason: String = null
    val pulling = new AtomicBoolean(false)
  }
  private val current = new ThreadLocal[Session]()

  private[runtime] def currentBinding: Option[StreamBinding] = Option(current.get())

  private def scoped[A](session: Session)(body: => A): A = {
    val previous = current.get()
    current.set(session)
    try body
    finally if (previous == null) current.remove() else current.set(previous)
  }

  def input[A](stream: AgentStream[A], encode: A => SchemaValue, lane: String, codec: PublicValueCodec.Codec): SchemaValue = {
    val session = Option(current.get()).getOrElse(
      throw BridgeException("stream values may only be encoded while starting an invocation")
    )
    val id = UUID.randomUUID().toString
    val consumer = session.registerInput(stream)
    session.inputs.put(id, Input(consumer, encode.asInstanceOf[Any => SchemaValue], lane, codec, session.pendingInputBytes))
    StreamReferenceValue(Some(id), None, Some(session))
  }

  def output[A](value: SchemaValue, decode: SchemaValue => A, lane: String, codec: PublicValueCodec.Codec): AgentStream[A] = {
    val reference = value match {
      case stream: StreamReferenceValue => stream
      case _ => throw BridgeException("expected stream reference")
    }
    val (_, token) = SchemaValueCodec.streamReference(reference)
    val key = token.getOrElse(throw BridgeException("server returned a provisional output stream"))
    reference.binding.getOrElse(throw BridgeException("output stream is detached from its invocation")).output(key, decode, lane, codec)
  }

  private final class Output[A](release: Int => Unit) extends AgentStream.Consumer[A] {
    private val waiting = new java.util.ArrayDeque[Promise[AgentStreamStep[A]]]()
    private val queued = new java.util.ArrayDeque[(Int, Either[() => Throwable, () => AgentStreamStep[A]])]()
    private var queuedBytes = 0L
    private var nextSequence = BigInt(0)
    private var checkpointSequence = BigInt(0)
    private var checkpointTerminal = false
    private var protocolTerminal = false
    private var consumerStopped = false
    private var pendingCancel: String = null
    private var failure: Throwable = null
    private val exposed = new AtomicBoolean(false)
    private var expectedLane: Option[String] = None
    private var observedLane: Option[String] = None
    @volatile var decode: SchemaValue => A = _
    @volatile var publicCodec: PublicValueCodec.Codec = _
    @volatile var cancelWith: String => Future[Unit] = _ => Future.successful(())
    private def evaluate(value: Either[() => Throwable, () => AgentStreamStep[A]]): Future[AgentStreamStep[A]] =
      value match {
        case Left(error) => scala.util.Try(error()).fold(Future.failed, Future.failed)
        case Right(item) => scala.util.Try(item()).fold(Future.failed, Future.successful)
      }
    def pull(): Future[AgentStreamStep[A]] = synchronized {
      if (!queued.isEmpty) {
        val (bytes, value) = queued.remove()
        queuedBytes -= bytes
        evaluate(value)
      }
      else if (failure != null) {
        val error = failure
        failure = null
        Future.failed(error)
      }
      else if (protocolTerminal || consumerStopped) Future.successful(End)
      else { val p = Promise[AgentStreamStep[A]](); waiting.add(p); p.future }
    }
    private def stop(reason: String): Future[Unit] = synchronized {
      while (!queued.isEmpty) { val (bytes, _) = queued.remove(); queuedBytes -= bytes; release(bytes) }
      while (!waiting.isEmpty) waiting.remove().success(End)
      if (consumerStopped || protocolTerminal) Future.successful(())
      else { consumerStopped = true; pendingCancel = reason; cancelWith(reason) }
    }
    def cancel(): Future[Unit] = stop("cancelled")
    def drop(): Future[Unit] = stop("consumer-drop")
    def fail(error: Throwable): Unit = synchronized {
      while (!queued.isEmpty) { val (bytes, _) = queued.remove(); queuedBytes -= bytes; release(bytes) }
      if (waiting.isEmpty && !consumerStopped) failure = error
      else while (!waiting.isEmpty) waiting.remove().failure(error)
      protocolTerminal = true
    }
    def offer(bytes: Int, value: Either[() => Throwable, () => AgentStreamStep[A]]): Unit = synchronized {
      if (protocolTerminal) { release(bytes); throw BridgeException("message received after output stream terminal") }
      if (consumerStopped) { release(bytes); return }
      if (waiting.isEmpty && (queued.size() >= StreamSessionProtocol.MaxQueuedItems || queuedBytes + bytes > StreamSessionProtocol.MaxQueuedBytes)) {
        release(bytes)
        throw BridgeException("output stream queue exceeded the protocol limit")
      }
      else if (waiting.isEmpty) { queued.add((bytes, value)); queuedBytes += bytes }
      else {
        val promise = waiting.remove()
        value match {
          case Left(error) => scala.util.Try(error()).fold(promise.failure, promise.failure)
          case Right(item) => scala.util.Try(item()).fold(promise.failure, promise.success)
        }
      }
    }
    def accept(first: BigInt, count: BigInt, finish: Boolean = false): Unit = synchronized {
      if (protocolTerminal || first != nextSequence) throw BridgeException("output stream sequence conflict")
      nextSequence = StreamSessionProtocol.checkedEnd(first, count)
    }
    def checkpoint(next: BigInt, terminal: Boolean = false): Unit = synchronized {
      if (next < checkpointSequence || next > nextSequence) throw BridgeException("output checkpoint sequence conflict")
      checkpointSequence = next
      if (terminal) checkpointTerminal = true
    }
    def prepareResume(): Unit = synchronized {
      while (!queued.isEmpty) { val (bytes, _) = queued.remove(); queuedBytes -= bytes; release(bytes) }
      nextSequence = checkpointSequence
      protocolTerminal = checkpointTerminal
    }
    def markTerminal(): Unit = synchronized { protocolTerminal = true; pendingCancel = null }
    def isTerminal: Boolean = synchronized(protocolTerminal)
    def remapCancel(send: String => Future[Unit]): Option[Future[Unit]] = synchronized {
      cancelWith = send
      Option(pendingCancel).map(send)
    }
    def observeLane(lane: String): Unit = synchronized {
      if (expectedLane.exists(_ != lane) || observedLane.exists(_ != lane))
        throw BridgeException("output stream lane changed")
      observedLane = Some(lane)
    }
    def setCodec(codec: SchemaValue => A, lane: String, publicValueCodec: PublicValueCodec.Codec): Unit = synchronized {
      if (observedLane.exists(_ != lane) || expectedLane.exists(_ != lane))
        throw BridgeException("output stream lane changed")
      expectedLane = Some(lane)
      decode = codec
      publicCodec = publicValueCodec
    }
    def expose(): Unit = if (!exposed.compareAndSet(false, true)) throw BridgeException("output stream was exposed more than once")
  }

  private final class Session(resolved: ResolvedAgent) extends StreamBinding {
    val inputs = new ConcurrentHashMap[String, Input]()
    private val inputIdentities = new java.util.IdentityHashMap[AgentStream[_], java.lang.Boolean]()
    val inputTokens = new ConcurrentHashMap[String, Input]()
    val outputs = new ConcurrentHashMap[String, Output[Any]]()
    val stableDirections = new ConcurrentHashMap[String, String]()
    val cursors = new ConcurrentHashMap[String, String]()
    val queuedBytes = new AtomicLong(0L)
    val pendingInputBytes = new AtomicLong(0L)

    def registerInput(stream: AgentStream[_]): Future[AgentStream.Consumer[Any]] = inputIdentities.synchronized {
      if (inputIdentities.put(stream, java.lang.Boolean.TRUE) != null)
        throw BridgeException("AgentStream was used at more than one structural coordinate")
      try AgentStream.claimAny(stream.asInstanceOf[AgentStream[Any]])
      catch {
        case error: Throwable =>
          inputIdentities.remove(stream)
          throw error
      }
    }

    def output[A](token: String, decode: SchemaValue => A, lane: String, codec: PublicValueCodec.Codec): AgentStream[A] = {
      val output = outputs.computeIfAbsent(token, _ => new Output[Any](bytes => queuedBytes.addAndGet(-bytes))).asInstanceOf[Output[A]]
      output.setCodec(decode, lane, codec)
      output.expose()
      AgentStream.remote(output)
    }
  }

  private[runtime] def invoke(
    resolved: ResolvedAgent,
    method: String,
    parameters: () => SchemaValue,
    constructorCodec: PublicValueCodec.Codec,
    inputCodec: PublicValueCodec.Codec,
    outputCodec: Option[PublicValueCodec.Codec],
    configCodecs: List[(List[String], PublicValueCodec.Codec)]
  ): Future[AgentInvocationResult] = {
    implicit val ec: ExecutionContext = resolved.configuration.executionContext
    val session = new Session(resolved)
    val encodedParameters = scoped(session)(parameters())
    val result = Promise[AgentInvocationResult]()
    val finished = Promise[Unit]()
    val channels = new ConcurrentHashMap[Long, String]()
    val directions = new ConcurrentHashMap[Long, String]()
    val tokenChannels = new ConcurrentHashMap[String, java.lang.Long]()
    @volatile var socket: WebSocket = null
    @volatile var accepted = false
    @volatile var sessionToken: String = null
    @volatile var pendingDescriptor: String = null
    @volatile var pendingAttempt: String = null
    @volatile var pendingResume = false
    @volatile var everAccepted = false
    val idempotencyKey = UUID.randomUUID.toString
    val reconnecting = new AtomicBoolean(false)
    val reconnectRequested = new AtomicBoolean(false)
    val reconnectAction = new AtomicReference[() => Unit](() => ())
    val pingSequence = new AtomicLong(0L)
    val pongSequence = new AtomicLong(0L)

    def send(text: String): Future[Unit] = Bridge.toScala(socket.sendText(text, true)).map(_ => ())
    def sendBinary(metadata: Json, payload: Vector[Byte]): Future[Unit] =
      Bridge.toScala(socket.sendBinary(StreamSessionProtocol.binary(metadata, payload), true)).map(_ => ())
    def cancel(channel: Long, reason: String): Future[Unit] = send(StreamSessionProtocol.message("streamCancel", Vector("channel" -> Json.fromLong(channel), "reason" -> Json.string(reason))))
    def highWater(mapping: Json): (BigInt, Boolean) = {
      val value = Json.requireField(mapping, "inputHighWater").fold(e => throw BridgeException(e), identity)
      StreamSessionProtocol.validateObject(value, Set("sequence", "terminal"), "inputHighWater")
      val sequence = StreamSessionProtocol.u64(value, "sequence")
      val terminal = Json.requireField(value, "terminal").flatMap(Json.asBoolean).fold(e => throw BridgeException(e), identity)
      sequence -> terminal
    }
    final case class Mapping(
      channel: Long,
      token: String,
      direction: String,
      provisional: Option[String],
      input: Option[Input],
      inputHighWater: Option[(BigInt, Boolean)]
    )
    def install(json: Json, complete: Boolean = false): Unit = Json.field(json, "mappings").foreach { value =>
      val mappings = Json.asArray(value).fold(e => throw BridgeException(e), identity)
      if (mappings.size > 4096) throw BridgeException("too many stream mappings")
      val messageChannels = collection.mutable.HashSet.empty[Long]
      val messageTokens = collection.mutable.HashSet.empty[String]
      val messageProvisionals = collection.mutable.HashSet.empty[String]
      val parsed = mappings.map { mapping =>
        StreamSessionProtocol.validateMapping(mapping)
        val channel = StreamSessionProtocol.channel(mapping)
        val token = Json.requireField(mapping, "streamToken").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
        val direction = Json.requireField(mapping, "direction").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
        if (!messageChannels.add(channel) || !messageTokens.add(token)) throw BridgeException("duplicate stream mapping")
        val provisional = Json.field(mapping, "provisionalRef").map { value =>
          val provisional = Json.asString(value).fold(e => throw BridgeException(e), identity)
          if (!messageProvisionals.add(provisional)) throw BridgeException("duplicate provisional stream mapping")
          provisional
        }
        StreamSessionProtocol.validateOpaqueToken(token, "stream token")
        if (direction != "input" && direction != "output") throw BridgeException("invalid stream direction")
        if (!session.stableDirections.containsKey(token) && session.stableDirections.size() >= 4096)
          throw BridgeException("too many stream mappings")
        val stableDirection = session.stableDirections.get(token)
        if (stableDirection != null && stableDirection != direction) throw BridgeException("stream token direction was rebound")
        val oldToken = channels.get(channel)
        if (oldToken != null && oldToken != token) throw BridgeException("stream channel was rebound")
        val oldDirection = directions.get(channel)
        if (oldDirection != null && oldDirection != direction) throw BridgeException("stream channel direction was rebound")
        val oldChannel = tokenChannels.get(token)
        if (oldChannel != null && oldChannel.longValue() != channel) throw BridgeException("stream token received two channels")
        val provisionalInput = provisional.flatMap(id => Option(session.inputs.get(id)))
        val knownInput = Option(session.inputTokens.get(token))
        if (provisional.nonEmpty && provisionalInput.isEmpty)
          throw BridgeException("input mapping contains an unknown provisional reference")
        if (direction == "input" && provisionalInput.orElse(knownInput).isEmpty)
          throw BridgeException("input mapping does not identify a registered stream")
        if (direction == "output" && provisional.nonEmpty)
          throw BridgeException("provisional input mapped as output")
        val input = provisionalInput.orElse(knownInput)
        input.foreach { in =>
          if (knownInput.exists(_ ne in)) throw BridgeException("input stream token was rebound")
          if (session.inputTokens.entrySet().asScala.exists(entry => entry.getKey != token && (entry.getValue eq in)))
            throw BridgeException("input stream was rebound to a different token")
        }
        Mapping(channel, token, direction, provisional, input, if (direction == "input") Some(highWater(mapping)) else None)
      }
      if ((session.stableDirections.keySet().asScala.toSet ++ messageTokens).size > 4096)
        throw BridgeException("too many stream mappings")
      if (complete) {
        if (pendingResume) {
          val knownTokens = session.stableDirections.keySet().asScala.toSet
          if (!knownTokens.subsetOf(messageTokens.toSet))
            throw BridgeException("resume acceptance omitted a known stream mapping")
        } else {
          val expectedProvisionals = session.inputs.keySet().asScala.toSet
          if (messageProvisionals.toSet != expectedProvisionals)
            throw BridgeException("initial acceptance omitted a provisional stream mapping")
        }
      }
      val pumps = collection.mutable.HashSet.empty[Input]
      parsed.foreach { mapping =>
        session.stableDirections.putIfAbsent(mapping.token, mapping.direction)
        channels.putIfAbsent(mapping.channel, mapping.token)
        directions.putIfAbsent(mapping.channel, mapping.direction)
        tokenChannels.putIfAbsent(mapping.token, java.lang.Long.valueOf(mapping.channel))
        if (mapping.direction == "output")
          session.outputs.computeIfAbsent(mapping.token, _ => new Output[Any](bytes => session.queuedBytes.addAndGet(-bytes)))
        mapping.input.foreach { in =>
          val previous = session.inputTokens.putIfAbsent(mapping.token, in)
          if (previous != null && (previous ne in)) throw BridgeException("input stream token was rebound")
          val (sequence, terminal) = mapping.inputHighWater.get
          val replay = in.state.remap(mapping.channel, sequence, terminal)
          replay.foreach { case (pending, mappedChannel) =>
            pending.send(mappedChannel).failed.foreach(_ => reconnectAction.get()())
          }
          if (terminal) in.cancelReason = null
          else if (in.cancelReason != null)
            cancel(mapping.channel, in.cancelReason).failed.foreach(_ => reconnectAction.get()())
          else if (replay.isEmpty) pumps.add(in)
        }
        Option(session.outputs.get(mapping.token)).flatMap(_.remapCancel(reason => cancel(mapping.channel, reason))).foreach(
          _.failed.foreach(_ => reconnectAction.get()())
        )
      }
      pumps.foreach(pump)
    }
    def sourceUnavailable(input: Input): Unit = {
      input.pulling.set(false)
      input.cancelReason = "source-unavailable"
      input.state.cancelLocal()
      val channel = input.state.currentChannel
      if (channel > 0) cancel(channel, input.cancelReason).failed.foreach(_ => reconnectAction.get()())
    }
    def pump(input: Input): Unit = if (input.pulling.compareAndSet(false, true)) {
      if (input.naturalEnd) {
        val first = input.state.nextSequence
        val pending = StreamSessionState.PendingTerminal(first, ch => send(StreamSessionProtocol.message("inputStreamEnd", Vector("channel" -> Json.fromLong(ch), "sequence" -> Json.string(first.toString)))))
        val (reserved, ch) = input.state.reserve(pending)
        input.pulling.set(false)
        if (ch > 0) reserved.send(ch).failed.foreach(_ => reconnectAction.get()())
        return
      }
      input.consumer.flatMap { consumer =>
        val buffered = input.bufferedPull
        val pulled = if (buffered == null) consumer.pull() else { input.bufferedPull = null; buffered }
        pulled.map(step => consumer -> step)
      }.onComplete {
        case Success((consumer, Item(value))) =>
          try {
            val first = input.state.nextSequence
            val encoded = scoped(session)(input.encode(value))
            val pending = input.lane match {
              case "u8" =>
                val builder = Vector.newBuilder[Byte]
                builder += SchemaValueCodec.asUByte(encoded).value.toByte
                var size = 1
                var filling = true
                while (filling && size < StreamSessionProtocol.MaxPackedBytes) {
                  val pulled = consumer.pull()
                  pulled.value match {
                    case Some(Success(Item(next))) => builder += SchemaValueCodec.asUByte(scoped(session)(input.encode(next))).value.toByte; size += 1
                    case Some(Success(End)) => input.naturalEnd = true; filling = false
                    case Some(Failure(error)) => throw error
                    case None => input.bufferedPull = pulled; filling = false
                  }
                }
                val bytes = builder.result()
                def pendingU8(start: BigInt, payload: Vector[Byte]): StreamSessionState.PendingInput =
                  StreamSessionState.PendingData(
                    start,
                    BigInt(payload.size),
                    payload.size,
                    ch => sendBinary(StreamSessionProtocol.inputBinaryMetadata("input-u8", ch, start, payload.size, None), payload),
                    Some(highWater => pendingU8(highWater, payload.drop((highWater - start).toInt)))
                  )
                pendingU8(first, bytes)
              case "binary" =>
                val (bytes, mime) = SchemaValueCodec.asBinary(encoded)
                StreamSessionProtocol.validateBinaryItem(bytes, mime)
                StreamSessionState.PendingData(first, 1, bytes.size, ch => sendBinary(StreamSessionProtocol.inputBinaryMetadata("input-binary", ch, first, 1, mime), bytes))
              case _ =>
                val valueJson = input.codec.encode(encoded)
                val bytes = valueJson.render.getBytes(java.nio.charset.StandardCharsets.UTF_8).length
                StreamSessionState.PendingData(first, 1, bytes, ch => send(StreamSessionProtocol.message("inputStreamItem", Vector("channel" -> Json.fromLong(ch), "sequence" -> Json.string(first.toString), "value" -> valueJson))))
            }
            val (reserved, ch) = input.state.reserve(pending)
            input.pulling.set(false)
            if (ch > 0) reserved.send(ch).failed.foreach(_ => reconnectAction.get()())
          } catch { case _: Throwable => sourceUnavailable(input) }
        case Success((_, End)) =>
          val first = input.state.nextSequence
          val pending = StreamSessionState.PendingTerminal(first, ch => send(StreamSessionProtocol.message("inputStreamEnd", Vector("channel" -> Json.fromLong(ch), "sequence" -> Json.string(first.toString)))))
          val (reserved, ch) = input.state.reserve(pending)
          input.pulling.set(false)
          if (ch > 0) reserved.send(ch).failed.foreach(_ => reconnectAction.get()())
        case Failure(_) => sourceUnavailable(input)
      }
    }

    def fatal(error: Throwable): Unit = {
      result.tryFailure(error)
      finished.tryFailure(error)
      session.outputs.values().asScala.foreach { output =>
        if (!output.isTerminal) output.fail(error)
      }
      session.inputTokens.values().asScala.foreach(_.consumer.foreach(_.cancel()))
      Option(socket).foreach(_.abort())
    }

    def closeNormally(): Unit =
      Option(socket).foreach(_.sendClose(WebSocket.NORMAL_CLOSURE, "complete"))

    def prepareResumeDescriptor(): Unit = {
      if (sessionToken == null) throw BridgeException("accepted session has no resume token")
      val cursors = Json.arr(session.cursors.values().asScala.toVector.sorted.map(Json.string))
      pendingAttempt = UUID.randomUUID.toString
      pendingDescriptor = StreamSessionProtocol.message("resumeAttach", Vector(
        "attemptId" -> Json.string(pendingAttempt), "operation" -> Json.string("resume"),
        "outputCursors" -> cursors, "sessionToken" -> Json.string(sessionToken)))
      pendingResume = true
      accepted = false
    }

    def scheduleLivenessPing(ws: WebSocket): Unit =
      CompletableFuture.delayedExecutor(LivenessPingDelayMillis, TimeUnit.MILLISECONDS).execute(() => {
        if ((ws eq socket) && !finished.isCompleted) {
          val sequence = pingSequence.incrementAndGet()
          val payload = ByteBuffer.allocate(java.lang.Long.BYTES)
          payload.putLong(sequence)
          payload.flip()
          try {
            Bridge.toScala(ws.sendPing(payload)).onComplete {
              case Failure(_) => if (ws eq socket) reconnectAction.get()()
              case Success(_) =>
                CompletableFuture.delayedExecutor(LivenessPingTimeoutMillis, TimeUnit.MILLISECONDS).execute(() => {
                  if ((ws eq socket) && !finished.isCompleted) {
                    if (pongSequence.get() < sequence) reconnectAction.get()()
                    else scheduleLivenessPing(ws)
                  }
                })
            }
          } catch {
            case _: Throwable => if (ws eq socket) reconnectAction.get()()
          }
        }
      })

    val listener = new WebSocket.Listener {
      private val texts = new ConcurrentHashMap[WebSocket, StringBuilder]()
      private val binaries = new ConcurrentHashMap[WebSocket, ByteArrayOutputStream]()
      override def onOpen(ws: WebSocket): Unit = { socket = ws; ws.request(1); scheduleLivenessPing(ws) }
      override def onText(ws: WebSocket, data: CharSequence, last: Boolean): CompletionStage[_] = {
        val text = texts.computeIfAbsent(ws, _ => new StringBuilder)
        text.append(data)
        if (text.length > StreamSessionProtocol.MaxMessageBytes) throw BridgeException("text frame exceeds the protocol limit")
        if (last) { texts.remove(ws); val raw = text.result(); if (ws eq socket) scoped(session)(handle(Json.parse(raw).fold(e => throw BridgeException(e), identity))) }
        ws.request(1); null
      }
      private def handle(json: Json): Unit = {
        val kind = Json.requireField(json, "type").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
        kind match {
          case "invocationAccepted" => StreamSessionProtocol.validate(json, kind, Set("attemptId", "idempotencyKey", "mappings", "sessionToken"))
          case "invocationRejected" => StreamSessionProtocol.validate(json, kind, Set("code", "message", "retryable"), Set("attemptId"))
          case "invocationResult" => StreamSessionProtocol.validate(json, kind, Set("mappings", "result"))
          case "outputStreamItem" => StreamSessionProtocol.validate(json, kind, Set("channel", "cursorToken", "mappings", "sequence", "value"))
          case "outputStreamEnd" => StreamSessionProtocol.validate(json, kind, Set("channel", "outcome", "sequence"), Set("cursorToken"))
          case "inputStreamAck" => StreamSessionProtocol.validate(json, kind, Set("channel", "highestContiguousSequence", "mappings", "terminal"))
          case "streamCancel" => StreamSessionProtocol.validate(json, kind, Set("channel", "reason"))
          case "attachmentRevoked" => StreamSessionProtocol.validate(json, kind, Set("reason"))
          case "invocationFinished" => StreamSessionProtocol.validate(json, kind, Set("outcome"))
          case _ => throw BridgeException(s"unsupported stream session message: $kind")
        }
        if (!accepted && kind != "invocationAccepted" && kind != "invocationRejected")
          throw BridgeException("message received before invocation acceptance")
        kind match {
          case "invocationAccepted" =>
            if (accepted) throw BridgeException("duplicate invocation acceptance")
            val attempt = Json.requireField(json, "attemptId").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
            val key = Json.requireField(json, "idempotencyKey").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
            if (attempt != pendingAttempt || key != idempotencyKey) throw BridgeException("acceptance does not match the pending operation")
            val token = Json.requireField(json, "sessionToken").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
            StreamSessionProtocol.validateOpaqueToken(token, "session token")
            install(json, complete = true)
            sessionToken = token
            accepted = true
            everAccepted = true
          case "invocationRejected" =>
            val attempt = Json.field(json, "attemptId").map(value => Json.asString(value).fold(e => throw BridgeException(e), identity))
            if (attempt.exists(_ != pendingAttempt) || (everAccepted && attempt.isEmpty))
              throw BridgeException("rejection does not match the pending operation")
            val code = Json.requireField(json, "code").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
            val message = Json.requireField(json, "message").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
            val retryable = Json.requireField(json, "retryable").flatMap(Json.asBoolean).fold(e => throw BridgeException(e), identity)
            val error = BridgeException(s"$code: $message")
            if (everAccepted && retryable) {
              try prepareResumeDescriptor()
              catch { case problem: Throwable => fatal(problem); return }
              Option(socket).foreach(_.abort())
              reconnectAction.get()()
            } else fatal(error)
          case "invocationResult" =>
            install(json)
            val resultValue = Json.requireField(json, "result").fold(e => throw BridgeException(e), identity)
            val resultKind = Json.requireField(resultValue, "kind").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
            val value = resultKind match {
              case "none" =>
                StreamSessionProtocol.validateObject(resultValue, Set("kind"), "invocation result")
                if (outputCodec.nonEmpty) throw BridgeException("method result is unexpectedly absent")
                None
              case "value" =>
                StreamSessionProtocol.validateObject(resultValue, Set("kind", "value"), "invocation result")
                Some(scoped(session)(outputCodec.getOrElse(throw BridgeException("unexpected invocation result value")).decode(Json.requireField(resultValue, "value").toOption.get)))
              case _ => throw BridgeException("invalid invocation result kind")
            }
            if (!result.trySuccess(AgentInvocationResult(resolved.agentId.getOrElse(AgentId("", "")), idempotencyKey, value, None)))
              throw BridgeException("duplicate invocation result")
          case "outputStreamItem" =>
            install(json); val ch = StreamSessionProtocol.channel(json)
            if (Option(directions.get(ch)) != Some("output")) throw BridgeException("invalid output channel")
            StreamSessionProtocol.u64(json, "sequence")
            val cursor = Json.requireField(json, "cursorToken").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
            StreamSessionProtocol.validateOpaqueToken(cursor, "cursor token")
            val raw = Json.requireField(json, "value").fold(e => throw BridgeException(e), identity)
            Option(channels.get(ch)).flatMap(token => Option(session.outputs.get(token)).map(token -> _)).foreach { case (token, o) =>
              o.observeLane("json")
              o.accept(StreamSessionProtocol.u64(json, "sequence"), 1)
              val bytes = json.render.getBytes(java.nio.charset.StandardCharsets.UTF_8).length
              if (session.queuedBytes.addAndGet(bytes) > StreamSessionProtocol.MaxSessionQueuedBytes) {
                session.queuedBytes.addAndGet(-bytes)
                throw BridgeException("session output queue exceeded the protocol limit")
              }
              val next = StreamSessionProtocol.checkedEnd(StreamSessionProtocol.u64(json, "sequence"), 1)
              o.offer(bytes, Right(() => {
                session.queuedBytes.addAndGet(-bytes)
                try {
                  val decoded = scoped(session)(o.decode(o.publicCodec.decode(raw)))
                  o.checkpoint(next)
                  session.cursors.put(token, cursor)
                  Item(decoded)
                } catch { case error: Throwable => fatal(error); throw error }
              }))
            }
          case "outputStreamEnd" =>
            val ch = StreamSessionProtocol.channel(json)
            if (Option(directions.get(ch)) != Some("output")) throw BridgeException("invalid output channel")
            StreamSessionProtocol.u64(json, "sequence")
            val outcome = Json.requireField(json, "outcome").toOption.get; val terminal = Json.requireField(outcome, "kind").flatMap(Json.asString).toOption.get
            terminal match {
              case "ok" => StreamSessionProtocol.validateObject(outcome, Set("kind"), "output terminal")
              case "error" => StreamSessionProtocol.validateObject(outcome, Set("kind", "code", "message"), "output terminal")
              case "cancelled" =>
                StreamSessionProtocol.validateObject(outcome, Set("kind", "reason"), "output terminal")
                val reason = Json.requireField(outcome, "reason").flatMap(Json.asString).toOption.get
                if (!Set("cancelled", "consumer-drop", "transport-detached", "source-unavailable", "producer-deleted", "invocation-failed", "protocol-error").contains(reason))
                  throw BridgeException("invalid output cancellation reason")
              case _ => throw BridgeException("invalid output terminal outcome")
            }
            val cursor = Json.field(json, "cursorToken").flatMap(v => Json.asString(v).toOption)
            cursor.foreach(StreamSessionProtocol.validateOpaqueToken(_, "cursor token"))
            Option(channels.get(ch)).flatMap(token => Option(session.outputs.get(token)).map(token -> _)).foreach { case (token, output) =>
              output.accept(StreamSessionProtocol.u64(json, "sequence"), 0, finish = true)
              val delivered = () => { cursor.foreach { value => output.checkpoint(StreamSessionProtocol.u64(json, "sequence"), terminal = true); session.cursors.put(token, value) }; End }
              terminal match {
                case "ok" => output.offer(0, Right(delivered))
                case "error" => output.offer(0, Left(() => { delivered(); AgentStreamError(Json.requireField(outcome, "code").flatMap(Json.asString).toOption.get, Json.requireField(outcome, "message").flatMap(Json.asString).toOption.get) }))
                case "cancelled" => output.offer(0, Left(() => { delivered(); AgentStreamCancelled(Json.requireField(outcome, "reason").flatMap(Json.asString).toOption.get) }))
                case _ => throw BridgeException("invalid output terminal outcome")
              }
              output.markTerminal()
            }
          case "streamCancel" =>
            val ch = StreamSessionProtocol.channel(json)
            if (Option(directions.get(ch)) != Some("input")) throw BridgeException("invalid input channel")
            val reason = Json.requireField(json, "reason").flatMap(Json.asString).toOption.get
            if (!Set("cancelled", "consumer-drop", "transport-detached", "source-unavailable", "producer-deleted", "invocation-failed", "protocol-error").contains(reason))
              throw BridgeException("invalid stream cancellation reason")
            Option(channels.get(ch)).flatMap(token => Option(session.inputTokens.get(token))).foreach { in =>
              in.state.cancel()
              in.consumer.foreach(_.cancel())
            }
          case "invocationFinished" =>
            val outcome = Json.requireField(json, "outcome").fold(e => throw BridgeException(e), identity)
            Json.requireField(outcome, "kind").flatMap(Json.asString).toOption.get match {
              case "success" =>
                StreamSessionProtocol.validateObject(outcome, Set("kind"), "invocation outcome")
                if (!result.isCompleted) throw BridgeException("invocation finished before its result")
                if (session.outputs.values().asScala.exists(output => !output.isTerminal)) throw BridgeException("invocation finished before an output terminal")
                if (!finished.trySuccess(())) throw BridgeException("duplicate invocation finish")
                closeNormally()
              case "failure" =>
                StreamSessionProtocol.validateObject(outcome, Set("kind", "code", "message"), "invocation outcome")
                val error = AgentStreamError(Json.requireField(outcome, "code").flatMap(Json.asString).toOption.get, Json.requireField(outcome, "message").flatMap(Json.asString).toOption.get)
                if (session.outputs.values().asScala.exists(output => !output.isTerminal)) throw BridgeException("invocation failure preceded an output terminal")
                result.tryFailure(error)
                if (!finished.trySuccess(())) throw BridgeException("duplicate invocation finish")
                closeNormally()
              case _ => throw BridgeException("invalid invocation outcome")
            }
          case "inputStreamAck" =>
            install(json)
            val ch = StreamSessionProtocol.channel(json)
            if (Option(directions.get(ch)) != Some("input")) throw BridgeException("invalid input ACK channel")
            val high = StreamSessionProtocol.u64(json, "highestContiguousSequence")
            val terminal = Json.requireField(json, "terminal").flatMap(Json.asBoolean).fold(e => throw BridgeException(e), identity)
            val in = Option(channels.get(ch)).flatMap(token => Option(session.inputTokens.get(token))).getOrElse(throw BridgeException("unknown input ACK channel"))
            if (in.state.acknowledge(high, terminal) && !terminal) pump(in)
          case "attachmentRevoked" =>
            if (Json.requireField(json, "reason").flatMap(Json.asString).toOption.get != "replaced")
              throw BridgeException("invalid attachment revocation reason")
            try prepareResumeDescriptor()
            catch { case error: Throwable => fatal(error); return }
            reconnectAction.get()()
          case other => result.tryFailure(BridgeException(s"unsupported stream session message: $other"))
        }
      }
      override def onError(ws: WebSocket, error: Throwable): Unit = if (ws eq socket) {
        error match {
          case protocol: BridgeException => fatal(protocol)
          case _ => reconnectAction.get()()
        }
      }
      override def onClose(ws: WebSocket, statusCode: Int, reason: String): CompletionStage[_] = {
        texts.remove(ws); binaries.remove(ws)
        if ((ws eq socket) && !finished.isCompleted) reconnectAction.get()()
        null
      }
      override def onPong(ws: WebSocket, data: ByteBuffer): CompletionStage[_] = {
        if ((ws eq socket) && data.remaining() == java.lang.Long.BYTES)
          pongSequence.set(data.getLong())
        ws.request(1)
        null
      }
      override def onBinary(ws: WebSocket, data: ByteBuffer, last: Boolean): CompletionStage[_] = {
        val binary = binaries.computeIfAbsent(ws, _ => new ByteArrayOutputStream())
        val chunk = new Array[Byte](data.remaining()); data.get(chunk); binary.write(chunk)
        if (binary.size() > StreamSessionProtocol.MaxMessageBytes) throw BridgeException("binary frame exceeds the protocol limit")
        if (!last) { ws.request(1); return null }
        binaries.remove(ws)
        if (!(ws eq socket)) { ws.request(1); return null }
        if (!accepted) throw BridgeException("binary message received before invocation acceptance")
        val (metadata, payload) = StreamSessionProtocol.decodeBinary(ByteBuffer.wrap(binary.toByteArray))
        val kind = Json.requireField(metadata, "kind").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
        val channel = StreamSessionProtocol.channel(metadata)
        if (Option(directions.get(channel)) != Some("output")) throw BridgeException("invalid output channel")
        val sequence = StreamSessionProtocol.u64(metadata, "sequence")
        val count = StreamSessionProtocol.u64(metadata, "itemCount")
        if (kind != "output-u8" && kind != "output-binary") throw BridgeException("invalid server binary lane")
        if ((kind == "output-u8" && (payload.isEmpty || payload.size > StreamSessionProtocol.MaxPackedBytes || count != payload.size)) ||
            (kind == "output-binary" && (payload.size > StreamSessionProtocol.MaxBinaryBytes || count != 1))) throw BridgeException("invalid binary stream item count")
        val cursor = Json.requireField(metadata, "cursorToken").flatMap(Json.asString).fold(e => throw BridgeException(e), identity)
        StreamSessionProtocol.validateOpaqueToken(cursor, "cursor token")
        val mapped = Option(channels.get(channel)).flatMap(token => Option(session.outputs.get(token)).map(token -> _)).getOrElse(throw BridgeException("unknown output channel"))
        mapped._2.observeLane(if (kind == "output-u8") "u8" else "binary")
        StreamSessionProtocol.validateBinary(metadata, kind)
        StreamSessionProtocol.checkedEnd(sequence, count)
        mapped._2.accept(sequence, count)
        if (session.queuedBytes.addAndGet(payload.size) > StreamSessionProtocol.MaxSessionQueuedBytes) {
          session.queuedBytes.addAndGet(-payload.size)
          throw BridgeException("session output queue exceeded the protocol limit")
        }
        if (kind == "output-u8") {
          val released = new AtomicBoolean(false)
          def releaseBatch(): Unit =
            if (released.compareAndSet(false, true)) session.queuedBytes.addAndGet(-payload.size)
          def delivery(index: Int): () => AgentStreamStep[Any] = () => {
            try {
              val raw = SchemaValue.U8Value(payload(index) & 0xff)
              val validated = mapped._2.publicCodec.decode(mapped._2.publicCodec.encode(raw))
              val decoded = scoped(session)(mapped._2.decode(validated))
              if (index + 1 < payload.size) mapped._2.offer(payload.size, Right(delivery(index + 1)))
              else {
                releaseBatch()
                mapped._2.checkpoint(StreamSessionProtocol.checkedEnd(sequence, count))
                session.cursors.put(mapped._1, cursor)
              }
              Item(decoded)
            } catch {
              case error: Throwable => releaseBatch(); fatal(error); throw error
            }
          }
          mapped._2.offer(payload.size, Right(delivery(0)))
        } else {
          val mime = Json.field(metadata, "mimeType").flatMap(v => Json.asString(v).toOption)
          mapped._2.offer(payload.size, Right(() => {
            session.queuedBytes.addAndGet(-payload.size)
            try {
              val raw = SchemaValue.BinaryValue(payload, mime)
              val decoded = scoped(session)(mapped._2.decode(mapped._2.publicCodec.decode(mapped._2.publicCodec.encode(raw))))
              mapped._2.checkpoint(StreamSessionProtocol.checkedEnd(sequence, count))
              session.cursors.put(mapped._1, cursor)
              Item(decoded)
            } catch { case error: Throwable => fatal(error); throw error }
          }))
        }
        ws.request(1); null
      }
    }
    val base = resolved.configuration.server.url.stripSuffix("/").replaceFirst("^http", "ws")
    val selector = Json.obj("agentType" -> Json.string(resolved.agentTypeName), "application" -> Json.string(resolved.configuration.appName), "constructorParameters" -> constructorCodec.encode(resolved.parameters), "environment" -> Json.string(resolved.configuration.envName), "method" -> Json.string(method))
    val codecsByPath = configCodecs.toMap
    val config = Json.arr(resolved.config.map { e =>
      val codec = codecsByPath.getOrElse(e.path, throw BridgeException(s"missing public config codec for ${e.path.mkString(".")}"))
      Json.obj("path" -> Json.arr(e.path.map(Json.string).toVector), "value" -> codec.encode(e.value))
    }.toVector)
    pendingAttempt = UUID.randomUUID.toString
    pendingDescriptor = StreamSessionProtocol.message("invocationStart", Vector("attemptId" -> Json.string(pendingAttempt), "config" -> config, "idempotencyKey" -> Json.string(idempotencyKey), "methodParameters" -> inputCodec.encode(encodedParameters), "selector" -> selector))

    def connect(): Future[Unit] =
      try {
        val builder = HttpClient.newHttpClient().newWebSocketBuilder().subprotocols(StreamSessionProtocol.Subprotocol).header("Authorization", s"Bearer ${resolved.configuration.server.token}")
        Bridge.toScala(builder.buildAsync(URI.create(s"$base/v1/agents/${StreamSessionProtocol.Endpoint}"), listener)).flatMap { ws =>
          if (ws.getSubprotocol != StreamSessionProtocol.Subprotocol) Future.failed(BridgeException("server did not select golem.agent-invocation.v1"))
          else { socket = ws; send(pendingDescriptor) }
        }
      } catch {
        case error: Throwable => Future.failed(error)
      }
    def delayReconnect(): Future[Unit] = {
      val delayed = Promise[Unit]()
      CompletableFuture.delayedExecutor(50, TimeUnit.MILLISECONDS).execute(() => delayed.success(()))
      delayed.future
    }
    def connectionCause(error: Throwable): Throwable = error match {
      case wrapped: CompletionException if wrapped.getCause != null => connectionCause(wrapped.getCause)
      case wrapped: ExecutionException if wrapped.getCause != null => connectionCause(wrapped.getCause)
      case other => other
    }
    def retryableConnectionFailure(error: Throwable): Boolean = connectionCause(error) match {
      case _: WebSocketHandshakeException => false
      case _: IOException => true
      case _: java.util.concurrent.TimeoutException => true
      case _ => false
    }
    def connectRetry(): Future[Unit] = connect().recoverWith {
      case error if retryableConnectionFailure(error) => delayReconnect().flatMap(_ => connectRetry())
      case error => Future.failed(connectionCause(error))
    }
    def reconnectSucceeded(): Unit = {
      reconnecting.set(false)
      if (reconnectRequested.get()) reconnectAction.get()()
    }
    reconnectAction.set(() => {
      reconnectRequested.set(true)
      if (reconnecting.compareAndSet(false, true)) {
        reconnectRequested.set(false)
        val previousSocket = socket
        socket = null
        Option(previousSocket).foreach(_.abort())
        channels.clear()
        directions.clear()
        tokenChannels.clear()
        session.inputs.values().asScala.foreach(_.state.detach())
        session.outputs.values().asScala.foreach(_.prepareResume())
        val ready = try {
          if (accepted) prepareResumeDescriptor()
          true
        } catch {
          case error: Throwable => fatal(error); false
        }
        if (ready) {
          connectRetry().onComplete {
            case scala.util.Success(_) => reconnectSucceeded()
            case scala.util.Failure(error) => reconnecting.set(false); fatal(error)
          }
        } else {
          reconnecting.set(false)
        }
      }
    })
    reconnecting.set(true)
    connectRetry().onComplete {
      case scala.util.Success(_) => reconnectSucceeded()
      case scala.util.Failure(error) => reconnecting.set(false); fatal(error)
    }
    result.future
  }
}
