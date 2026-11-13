package golem.bridge.runtime

import golem.bridge.runtime.json.Json
import java.net.{ServerSocket, Socket}
import java.nio.charset.StandardCharsets.UTF_8
import java.security.MessageDigest
import java.util.Base64
import scala.concurrent.{Await, Future, Promise}
import scala.concurrent.ExecutionContext.Implicits.global
import scala.concurrent.duration.*

class StreamRuntimeTest extends munit.FunSuite {
  // A real RFC 6455 peer, including masked client frames and liveness pongs.
  private class Peer(val socket: Socket) {
    socket.setSoTimeout(10000)
    private val in = socket.getInputStream
    private val out = socket.getOutputStream
    private val request = new StringBuilder
    while (!request.toString.endsWith("\r\n\r\n")) request.append(in.read().toChar)
    private val key = request.toString.split("\r\n").find(_.toLowerCase.startsWith("sec-websocket-key:")).get.split(":", 2)(1).trim
    private val accept = Base64.getEncoder.encodeToString(MessageDigest.getInstance("SHA-1").digest((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").getBytes(UTF_8)))
    out.write(s"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Protocol: golem.agent-invocation.v1\r\nSec-WebSocket-Accept: $accept\r\n\r\n".getBytes(UTF_8))
    out.flush()
    private def sendFrame(op: Int, bytes: Array[Byte]): Unit = synchronized {
      out.write(128 | op)
      if (bytes.length < 126) out.write(bytes.length)
      else if (bytes.length <= 65535) { out.write(126); out.write(bytes.length >> 8); out.write(bytes.length & 255) }
      else { out.write(127); out.write(java.nio.ByteBuffer.allocate(8).putLong(bytes.length.toLong).array()) }
      out.write(bytes); out.flush()
    }
    def send(value: Json): Unit = sendFrame(1, value.render.getBytes(UTF_8))
    private def receiveFrame(): (Int, Array[Byte]) = {
      val header = in.read()
      if (header < 0) throw new java.io.EOFException("WebSocket peer closed")
      val op = header & 15
      val size = in.read() & 127
      val length = if (size == 126) (in.read() << 8) | in.read() else size
      val mask = in.readNBytes(4)
      val bytes = in.readNBytes(length)
      val decoded = bytes.indices.map(i => (bytes(i) ^ mask(i % 4)).toByte).toArray
      if (op == 9) { sendFrame(10, decoded); receiveFrame() }
      else (op, decoded)
    }
    def receive(): Json = Json.parse(new String(receiveFrame()._2, UTF_8)).toOption.get
    def receiveBinary(): (Json, Vector[Byte]) = {
      val (op, bytes) = receiveFrame()
      assertEquals(op, 2)
      StreamSessionProtocol.decodeBinary(java.nio.ByteBuffer.wrap(bytes))
    }
    def close(): Unit = socket.close()
  }
  private def field(value: Json, name: String): Json = Json.requireField(value, name).toOption.get
  private def text(value: Json, name: String): String = Json.asString(field(value, name)).toOption.get
  private def message(kind: String, fields: (String, Json)*): Json =
    Json.parse(StreamSessionProtocol.message(kind, fields.toVector)).fold(e => throw new IllegalArgumentException(e), identity)

  for (rejectAfterTakeover <- List(false, true); terminal <- List("finished", "failed", "consumer-cancelled")) test(s"resume switches to takeover and retries its uncertain descriptor ($rejectAfterTakeover, $terminal)") {
    val server = new ServerSocket(0)
    val peerDone = Promise[Unit]()
    val consumed = Promise[Unit]()
    val graph = """{"root":{"kind":"stream","value":{"inner":{"kind":"string","value":{}}}}}"""
    val mapping = Json.obj("channel" -> Json.fromLong(1), "direction" -> Json.string("output"), "streamToken" -> Json.string("output-token"))
    val mappings = Json.arr(Vector(mapping))
    val peer = Future {
      var sockets = List.empty[Peer]
      def connect(): Peer = { val p = new Peer(server.accept()); sockets ::= p; p }
      try {
        val startPeer = connect()
        val start = startPeer.receive()
        val key = field(start, "idempotencyKey")
        def accepted(p: Peer, descriptor: Json): Unit = p.send(message("invocationAccepted",
          "attemptId" -> field(descriptor, "attemptId"), "idempotencyKey" -> key,
          "sessionToken" -> Json.string("stable-session"), "mappings" -> mappings))
        accepted(startPeer, start)
        startPeer.send(message("invocationResult", "result" -> Json.obj("kind" -> Json.string("value"),
          "graph" -> Json.parse(graph).toOption.get, "value" -> Json.obj("kind" -> Json.string("stream"), "value" -> Json.obj("streamToken" -> Json.string("output-token")))), "mappings" -> mappings))
        startPeer.send(message("outputStreamItem", "channel" -> Json.fromLong(1), "sequence" -> Json.string("0"),
          "cursorToken" -> Json.string("delivered-cursor"), "value" -> Json.obj("kind" -> Json.string("string"), "value" -> Json.string("before")), "mappings" -> mappings))
        Await.result(consumed.future, 10.seconds)
        startPeer.close()
        val resumePeer = connect()
        val resume = resumePeer.receive()
        assertEquals(text(resume, "operation"), "resume")
        assertEquals(field(resume, "outputCursors").render, Json.arr(Vector(Json.string("delivered-cursor"))).render)
        resumePeer.send(message("invocationRejected", "attemptId" -> field(resume, "attemptId"),
          "code" -> Json.string("invalid-attachment-state"), "message" -> Json.string("old attachment remains"), "retryable" -> Json.bool(false)))
        val takeoverPeer = connect()
        val takeover = takeoverPeer.receive()
        assertEquals(text(takeover, "operation"), "takeover")
        assertNotEquals(field(takeover, "attemptId").render, field(resume, "attemptId").render)
        assertEquals(field(takeover, "sessionToken").render, field(resume, "sessionToken").render)
        assertEquals(field(takeover, "outputCursors").render, field(resume, "outputCursors").render)
        takeoverPeer.close()
        val retryPeer = connect()
        val retry = retryPeer.receive()
        assertEquals(retry.render, takeover.render)
        accepted(retryPeer, retry)
        val finalPeer = if (rejectAfterTakeover) {
          retryPeer.send(message("invocationRejected", "attemptId" -> field(retry, "attemptId"),
            "code" -> Json.string("routing-miss"), "message" -> Json.string("retry"), "retryable" -> Json.bool(true)))
          val nextPeer = connect()
          val next = nextPeer.receive()
          assertEquals(text(next, "operation"), "resume")
          assertNotEquals(field(next, "attemptId").render, field(retry, "attemptId").render)
          accepted(nextPeer, next)
          nextPeer
        } else retryPeer
        finalPeer.send(message("outputStreamItem", "channel" -> Json.fromLong(1), "sequence" -> Json.string("1"),
          "cursorToken" -> Json.string("next-cursor"), "value" -> Json.obj("kind" -> Json.string("string"), "value" -> Json.string("after")), "mappings" -> mappings))
        val outcome = terminal match {
          case "failed" => Json.obj("kind" -> Json.string("failed"), "error" -> Json.obj("kind" -> Json.string("interrupted"), "message" -> Json.string("producer diagnostic")))
          case "consumer-cancelled" => Json.obj("kind" -> Json.string("consumer-cancelled"), "details" -> Json.string("reader diagnostic"))
          case _ => Json.obj("kind" -> Json.string("finished"))
        }
        finalPeer.send(message("stream-end", "channel" -> Json.fromLong(1), "direction" -> Json.string("output"), "sequence" -> Json.string("2"),
          "cursorToken" -> Json.string("terminal-cursor"), "outcome" -> outcome))
        finalPeer.send(message("invocationFinished", "outcome" -> Json.obj("kind" -> Json.string("success"))))
        peerDone.success(())
      } catch { case e: Throwable => peerDone.tryFailure(e); throw e }
      finally { sockets.foreach(_.close()); server.close() }
    }
    val empty = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"tuple","value":{"elements":[]}}}""")
    val resolved = ResolvedAgent(Configuration(GolemServer.Custom(s"http://127.0.0.1:${server.getLocalPort}", "token"), "app", "env", global),
      "RecoveryAgent", SchemaValue.TupleValue(Nil), None, Nil, None)
    try {
      val result = Await.result(Bridge.invokeStreamingAgent(resolved, "run", () => SchemaValue.TupleValue(Nil), empty, empty, Some(PublicValueCodec.fromSchemaGraphJson(graph)), Nil), 15.seconds)
      val stream = StreamSession.output(result.result.get, SchemaValueCodec.asString, "json",
        PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"string","value":{}}}"""))
      val consumer = Await.result(stream.consume(), 5.seconds)
      assertEquals(Await.result(consumer.pull(), 5.seconds), AgentStreamStep.Item("before"))
      consumed.success(())
      Await.result(peerDone.future, 15.seconds)
      assertEquals(Await.result(consumer.pull(), 15.seconds), AgentStreamStep.Item("after"))
      terminal match {
        case "finished" => assertEquals(Await.result(consumer.pull(), 5.seconds), AgentStreamStep.End)
        case other =>
          val error = intercept[AgentStreamEnded](Await.result(consumer.pull(), 5.seconds))
          val expected = if (other == "failed") StreamOutcome.Failed(StreamError.Interrupted("producer diagnostic"))
            else StreamOutcome.ConsumerCancelled(Some("reader diagnostic"))
          assertEquals(error.outcome, expected)
      }
    } finally server.close()
  }

  for (failSource <- List(false, true)) test(s"input terminal is sequenced and replayed after transport loss ($failSource)") {
    val server = new ServerSocket(0)
    val peer = Future {
      var sockets = List.empty[Peer]
      def connect(): Peer = { val p = new Peer(server.accept()); sockets ::= p; p }
      try {
        val first = connect()
        val start = first.receive()
        val provisional = field(field(start, "methodParameters"), "value")
        val resumedMappings = Json.arr(Vector(Json.obj("channel" -> Json.fromLong(9), "direction" -> Json.string("input"),
          "streamToken" -> Json.string("input-token"),
          "inputHighWater" -> Json.obj("sequence" -> Json.string("1"), "terminal" -> Json.bool(false)))))
        val initialMappings = Json.arr(Vector(Json.obj("channel" -> Json.fromLong(1), "direction" -> Json.string("input"),
          "streamToken" -> Json.string("input-token"), "provisionalRef" -> field(provisional, "provisionalRef"),
          "inputHighWater" -> Json.obj("sequence" -> Json.string("0"), "terminal" -> Json.bool(false)))))
        def accept(p: Peer, descriptor: Json, streams: Json): Unit = p.send(message("invocationAccepted",
          "attemptId" -> field(descriptor, "attemptId"), "idempotencyKey" -> field(start, "idempotencyKey"),
          "sessionToken" -> Json.string("input-session"), "mappings" -> streams))
        accept(first, start, initialMappings)
        assertEquals(text(first.receive(), "type"), "inputStreamItem")
        first.send(message("inputStreamAck", "channel" -> Json.fromLong(1), "highestContiguousSequence" -> Json.string("1"),
          "terminal" -> Json.bool(false), "mappings" -> Json.arr(Vector.empty)))
        val terminal = first.receive()
        assertEquals(text(terminal, "type"), "stream-end")
        assertEquals(text(terminal, "direction"), "input")
        assertEquals(text(terminal, "sequence"), "1")
        val expected = if (failSource) StreamOutcome.Failed(StreamError.Interrupted("local source diagnostic")) else StreamOutcome.Finished
        assertEquals(StreamSessionProtocol.streamOutcome(field(terminal, "outcome")), expected)
        first.close()
        val resumed = connect()
        val descriptor = resumed.receive()
        assertEquals(text(descriptor, "type"), "resumeAttach")
        accept(resumed, descriptor, resumedMappings)
        val replay = resumed.receive()
        assertEquals(StreamSessionProtocol.channel(replay), 9L)
        assertEquals(text(replay, "sequence"), "1")
        assertEquals(field(replay, "outcome"), field(terminal, "outcome"))
        resumed.send(message("inputStreamAck", "channel" -> Json.fromLong(9), "highestContiguousSequence" -> Json.string("1"),
          "terminal" -> Json.bool(true), "mappings" -> Json.arr(Vector.empty)))
        resumed.send(message("invocationResult", "result" -> Json.obj("kind" -> Json.string("none")), "mappings" -> Json.arr(Vector.empty)))
        resumed.send(message("invocationFinished", "outcome" -> Json.obj("kind" -> Json.string("success"))))
      } finally { sockets.foreach(_.close()); server.close() }
    }
    val empty = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"tuple","value":{"elements":[]}}}""")
    val stringCodec = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"string","value":{}}}""")
    val streamCodec = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"stream","value":{"inner":{"kind":"string","value":{}}}}}""")
    var pulled = false
    val source = AgentStream.fromPull[String](() => {
      if (!pulled) { pulled = true; Future.successful(AgentStreamStep.Item("first")) }
      else if (failSource) Future.failed(new IllegalStateException("local source diagnostic"))
      else Future.successful(AgentStreamStep.End)
    })
    val resolved = ResolvedAgent(Configuration(GolemServer.Custom(s"http://127.0.0.1:${server.getLocalPort}", "token"), "app", "env", global),
      "RecoveryAgent", SchemaValue.TupleValue(Nil), None, Nil, None)
    try {
      Await.result(Bridge.invokeStreamingAgent(resolved, "run", () => StreamSession.input(source, SchemaValue.StringValue.apply, "json", stringCodec),
        empty, streamCodec, None, Nil), 15.seconds)
      Await.result(peer, 15.seconds)
    } finally server.close()
  }

  for (synchronousThrow <- List(false, true)) test(s"packed input preserves the prefix before an immediate source failure ($synchronousThrow)") {
    val server = new ServerSocket(0)
    val peer = Future {
      var sockets = List.empty[Peer]
      def connect(): Peer = { val p = new Peer(server.accept()); sockets ::= p; p }
      try {
        val first = connect()
        val start = first.receive()
        val provisional = field(field(start, "methodParameters"), "value")
        def accept(p: Peer, descriptor: Json, channel: Long, sequence: String, initial: Boolean): Unit = {
          val mapping = Json.obj(Vector("channel" -> Json.fromLong(channel), "direction" -> Json.string("input"),
            "streamToken" -> Json.string("input-token"), "inputHighWater" -> Json.obj("sequence" -> Json.string(sequence), "terminal" -> Json.bool(false))) ++
            (if (initial) Vector("provisionalRef" -> field(provisional, "provisionalRef")) else Vector.empty))
          p.send(message("invocationAccepted", "attemptId" -> field(descriptor, "attemptId"), "idempotencyKey" -> field(start, "idempotencyKey"),
            "sessionToken" -> Json.string("input-session"), "mappings" -> Json.arr(Vector(mapping))))
        }
        accept(first, start, 1, "0", true)
        val (prefix, bytes) = first.receiveBinary()
        assertEquals(text(prefix, "sequence"), "0")
        assertEquals(bytes, Vector[Byte](7, 11, 19))
        first.close()
        val resumed = connect()
        val descriptor = resumed.receive()
        accept(resumed, descriptor, 9, "0", false)
        val (replayed, replayedBytes) = resumed.receiveBinary()
        assertEquals(StreamSessionProtocol.channel(replayed), 9L)
        assertEquals(text(replayed, "sequence"), "0")
        assertEquals(replayedBytes, bytes)
        resumed.send(message("inputStreamAck", "channel" -> Json.fromLong(9), "highestContiguousSequence" -> Json.string("3"),
          "terminal" -> Json.bool(false), "mappings" -> Json.arr(Vector.empty)))
        val terminal = resumed.receive()
        assertEquals(text(terminal, "type"), "stream-end")
        assertEquals(text(terminal, "sequence"), "3")
        assertEquals(StreamSessionProtocol.streamOutcome(field(terminal, "outcome")), StreamOutcome.Failed(StreamError.Interrupted("packed source diagnostic")))
        resumed.close()
        val again = connect()
        accept(again, again.receive(), 17, "3", false)
        val replayedTerminal = again.receive()
        assertEquals(StreamSessionProtocol.channel(replayedTerminal), 17L)
        assertEquals(text(replayedTerminal, "sequence"), "3")
        assertEquals(field(replayedTerminal, "outcome"), field(terminal, "outcome"))
        again.send(message("inputStreamAck", "channel" -> Json.fromLong(17), "highestContiguousSequence" -> Json.string("3"),
          "terminal" -> Json.bool(true), "mappings" -> Json.arr(Vector.empty)))
        again.send(message("invocationResult", "result" -> Json.obj("kind" -> Json.string("none")), "mappings" -> Json.arr(Vector.empty)))
        again.send(message("invocationFinished", "outcome" -> Json.obj("kind" -> Json.string("success"))))
      } finally { sockets.foreach(_.close()); server.close() }
    }
    val empty = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"tuple","value":{"elements":[]}}}""")
    val codec = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"u8","value":{}}}""")
    val streamCodec = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"stream","value":{"inner":{"kind":"u8","value":{}}}}}""")
    var values = List(7, 11, 19)
    val source = AgentStream.fromPull[Int](() => values match {
      case head :: tail => values = tail; Future.successful(AgentStreamStep.Item(head))
      case Nil =>
        val error = new IllegalStateException("packed source diagnostic")
        if (synchronousThrow) throw error else Future.failed(error)
    })
    val resolved = ResolvedAgent(Configuration(GolemServer.Custom(s"http://127.0.0.1:${server.getLocalPort}", "token"), "app", "env", global),
      "RecoveryAgent", SchemaValue.TupleValue(Nil), None, Nil, None)
    try {
      Await.result(Bridge.invokeStreamingAgent(resolved, "run", () => StreamSession.input(source, SchemaValue.U8Value.apply, "u8", codec),
        empty, streamCodec, None, Nil), 15.seconds)
      Await.result(peer, 15.seconds)
    } finally server.close()
  }

  test("output consumer disposal sends a directional consumer terminal at sequence zero") {
    val server = new ServerSocket(0)
    val graph = """{"root":{"kind":"stream","value":{"inner":{"kind":"string","value":{}}}}}"""
    val peer = Future {
      val p = new Peer(server.accept())
      try {
        val start = p.receive()
        val mappings = Json.arr(Vector(Json.obj("channel" -> Json.fromLong(7), "direction" -> Json.string("output"), "streamToken" -> Json.string("output-token"))))
        p.send(message("invocationAccepted", "attemptId" -> field(start, "attemptId"), "idempotencyKey" -> field(start, "idempotencyKey"),
          "sessionToken" -> Json.string("session"), "mappings" -> mappings))
        p.send(message("invocationResult", "result" -> Json.obj("kind" -> Json.string("value"), "graph" -> Json.parse(graph).toOption.get,
          "value" -> Json.obj("kind" -> Json.string("stream"), "value" -> Json.obj("streamToken" -> Json.string("output-token")))), "mappings" -> mappings))
        val terminal = p.receive()
        assertEquals(text(terminal, "type"), "stream-end")
        assertEquals(text(terminal, "direction"), "output")
        assertEquals(text(terminal, "sequence"), "0")
        assertEquals(StreamSessionProtocol.channel(terminal), 7L)
        assertEquals(StreamSessionProtocol.streamOutcome(field(terminal, "outcome")), StreamOutcome.ConsumerCancelled(Some("consumer-drop")))
        p.send(message("stream-end", "channel" -> Json.fromLong(7), "direction" -> Json.string("output"), "sequence" -> Json.string("0"),
          "outcome" -> Json.obj("kind" -> Json.string("consumer-cancelled"))))
        p.send(message("invocationFinished", "outcome" -> Json.obj("kind" -> Json.string("success"))))
      } finally { p.close(); server.close() }
    }
    val empty = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"tuple","value":{"elements":[]}}}""")
    val resolved = ResolvedAgent(Configuration(GolemServer.Custom(s"http://127.0.0.1:${server.getLocalPort}", "token"), "app", "env", global),
      "RecoveryAgent", SchemaValue.TupleValue(Nil), None, Nil, None)
    try {
      val result = Await.result(Bridge.invokeStreamingAgent(resolved, "run", () => SchemaValue.TupleValue(Nil), empty, empty,
        Some(PublicValueCodec.fromSchemaGraphJson(graph)), Nil), 15.seconds)
      val stream = StreamSession.output(result.result.get, SchemaValueCodec.asString, "json",
        PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"string","value":{}}}"""))
      Await.result(Await.result(stream.consume(), 5.seconds).drop(), 5.seconds)
      Await.result(peer, 10.seconds)
    } finally server.close()
  }

  for (mode <- List("immediate", "delayed", "combined", "cancel-before-failure", "cancel-after-failure", "session-cancel", "session-finish")) test(s"buffered packed failures are admitted while prefix ACKs are withheld ($mode)") {
    val server = new ServerSocket(0)
    val complete = Promise[Unit]()
    val prefixReceived = Promise[Unit]()
    val failures = List.fill(if (mode == "combined") 3 else 1)(Promise[AgentStreamStep[Int]]())
    val pulled = failures.map(_ => Promise[Unit]())
    val cancelled = failures.map(_ => Promise[Unit]())
    val diagnostic = "x" * (if (mode == "combined") 12 * 1024 * 1024 else if (mode == "cancel-after-failure") 1024 else StreamSessionProtocol.MaxReplayBytes + 1)
    val peer = Future {
      val p = new Peer(server.accept())
      try {
        val start = p.receive()
        val refs = field(field(start, "methodParameters"), "value")
        val entries = Json.asArray(field(refs, "elements")).toOption.get
        val mappings = entries.zipWithIndex.map { case (entry, index) => Json.obj("channel" -> Json.fromInt(index + 1),
          "direction" -> Json.string("input"), "streamToken" -> Json.string(s"input-$index"),
          "provisionalRef" -> field(field(entry, "value"), "provisionalRef"),
          "inputHighWater" -> Json.obj("sequence" -> Json.string("0"), "terminal" -> Json.bool(false))) }
        p.send(message("invocationAccepted", "attemptId" -> field(start, "attemptId"), "idempotencyKey" -> field(start, "idempotencyKey"),
          "sessionToken" -> Json.string("session"), "mappings" -> Json.arr(mappings)))
        if (mode != "immediate") {
          failures.indices.foreach { _ => assertEquals(p.receiveBinary()._2, Vector[Byte](7)) }
          pulled.foreach(signal => Await.result(signal.future, 5.seconds))
          prefixReceived.success(())
          if (mode == "session-finish") {
            p.send(message("invocationResult", "result" -> Json.obj("kind" -> Json.string("none")), "mappings" -> Json.arr(Vector.empty)))
            p.send(message("invocationFinished", "outcome" -> Json.obj("kind" -> Json.string("success"))))
          }
          if (mode.startsWith("session")) Await.result(cancelled.head.future, 5.seconds)
          if (mode == "cancel-before-failure") {
            p.send(message("stream-end", "channel" -> Json.fromInt(1), "direction" -> Json.string("input"), "sequence" -> Json.string("0"),
              "outcome" -> Json.obj("kind" -> Json.string("consumer-cancelled"))))
            Await.result(cancelled.head.future, 5.seconds)
          }
          failures.foreach(_.failure(new IllegalStateException(diagnostic)))
          if (mode == "cancel-after-failure") {
            p.send(message("stream-end", "channel" -> Json.fromInt(1), "direction" -> Json.string("input"), "sequence" -> Json.string("0"),
              "outcome" -> Json.obj("kind" -> Json.string("consumer-cancelled"))))
            Await.result(cancelled.head.future, 5.seconds)
          }
          if (mode.startsWith("cancel")) {
            p.send(message("invocationResult", "result" -> Json.obj("kind" -> Json.string("none")), "mappings" -> Json.arr(Vector.empty)))
            p.send(message("invocationFinished", "outcome" -> Json.obj("kind" -> Json.string("success"))))
          }
        }
        Await.result(complete.future, 5.seconds)
      } finally { p.close(); server.close() }
    }
    val empty = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"tuple","value":{"elements":[]}}}""")
    val codec = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"u8","value":{}}}""")
    val streamType = Json.parse("""{"kind":"stream","value":{"inner":{"kind":"u8","value":{}}}}""").toOption.get
    val inputGraph = Json.obj("root" -> Json.obj("kind" -> Json.string("tuple"), "value" -> Json.obj("elements" -> Json.arr(Vector.fill(failures.size)(streamType)))))
    val sources = failures.indices.map { index =>
      var first = true
      AgentStream.fromPull[Int](() => {
        if (first) { first = false; Future.successful(AgentStreamStep.Item(7)) }
        else {
          pulled(index).trySuccess(())
          if (mode == "immediate") Future.failed(new IllegalStateException(diagnostic)) else failures(index).future
        }
      }, () => { cancelled(index).trySuccess(()); Future.unit })
    }
    val resolved = ResolvedAgent(Configuration(GolemServer.Custom(s"http://127.0.0.1:${server.getLocalPort}", "token"), "app", "env", global),
      "RecoveryAgent", SchemaValue.TupleValue(Nil), None, Nil, None)
    try {
      val (invocation, cancellation) = Bridge.invokeStreamingAgentCancelable(resolved, "run", () => SchemaValue.TupleValue(sources.map(source =>
        StreamSession.input(source, SchemaValue.U8Value.apply, "u8", codec)).toList), empty, PublicValueCodec.fromSchemaGraphJson(inputGraph.render), None, Nil)
      val checked = Future.firstCompletedOf(List(invocation, peer.flatMap(_ => Promise[AgentInvocationResult]().future)))
      if (mode == "session-cancel") {
        Await.result(prefixReceived.future, 5.seconds)
        cancellation.cancel()
        intercept[BridgeCancelledException](Await.result(checked, 5.seconds))
      } else if (mode.startsWith("cancel") || mode == "session-finish") {
        try Await.result(checked, 5.seconds)
        catch { case error: java.util.concurrent.TimeoutException =>
          throw new IllegalStateException(s"prefix=${prefixReceived.isCompleted}, pulled=${pulled.map(_.isCompleted)}, cancelled=${cancelled.map(_.isCompleted)}, peer=${peer.isCompleted}", error)
        }
      }
      else assert(intercept[BridgeException](Await.result(checked, 5.seconds)).getMessage.contains("unacknowledged input exceeds"))
      complete.success(())
      Await.result(peer, 5.seconds)
    } finally { complete.trySuccess(()); server.close() }
  }

  test("an oversized public input failure diagnostic fails replay admission instead of remaining uncharged") {
    val server = new ServerSocket(0)
    val finished = Promise[Unit]()
    val peer = Future {
      val p = new Peer(server.accept())
      try {
        val start = p.receive()
        val provisional = field(field(start, "methodParameters"), "value")
        val mapping = Json.obj("channel" -> Json.fromInt(1), "direction" -> Json.string("input"), "streamToken" -> Json.string("input-token"),
          "provisionalRef" -> field(provisional, "provisionalRef"), "inputHighWater" -> Json.obj("sequence" -> Json.string("0"), "terminal" -> Json.bool(false)))
        p.send(message("invocationAccepted", "attemptId" -> field(start, "attemptId"), "idempotencyKey" -> field(start, "idempotencyKey"),
          "sessionToken" -> Json.string("session"), "mappings" -> Json.arr(Vector(mapping))))
        Await.result(finished.future, 10.seconds)
      } finally { p.close(); server.close() }
    }
    val empty = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"tuple","value":{"elements":[]}}}""")
    val codec = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"u8","value":{}}}""")
    val streamCodec = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"stream","value":{"inner":{"kind":"u8","value":{}}}}}""")
    val source = AgentStream.fromPull[Int](() => Future.failed(new IllegalStateException("x" * (StreamSessionProtocol.MaxReplayBytes + 1))))
    val resolved = ResolvedAgent(Configuration(GolemServer.Custom(s"http://127.0.0.1:${server.getLocalPort}", "token"), "app", "env", global),
      "RecoveryAgent", SchemaValue.TupleValue(Nil), None, Nil, None)
    try {
      val error = intercept[BridgeException](Await.result(Bridge.invokeStreamingAgent(resolved, "run",
        () => StreamSession.input(source, SchemaValue.U8Value.apply, "u8", codec), empty, streamCodec, None, Nil), 10.seconds))
      assert(error.getMessage.contains("unacknowledged input exceeds"))
      finished.success(())
      Await.result(peer, 10.seconds)
    } finally { finished.trySuccess(()); server.close() }
  }

  for (mode <- List("stream-limit", "session-limit", "deliver", "dispose")) test(s"typed output terminals use queue budgets and release admitted payloads ($mode)") {
    val server = new ServerSocket(0)
    val released = Promise[Unit]()
    val count = if (mode == "stream-limit") 1 else 3
    val elementCount = if (mode == "stream-limit") 32 else 24
    val chunkSize = 524270
    val stringGraph = Json.parse("""{"root":{"kind":"string","value":{}}}""").toOption.get
    val failureGraph = Json.parse("""{"root":{"kind":"list","value":{"element":{"kind":"string","value":{}}}}}""").toOption.get
    val streamType = Json.parse("""{"kind":"stream","value":{"inner":{"kind":"string","value":{}}}}""").toOption.get
    val graph = Json.obj("root" -> Json.obj("kind" -> Json.string("tuple"), "value" -> Json.obj("elements" -> Json.arr(Vector.fill(count)(streamType)))))
    val peer = Future {
      val p = new Peer(server.accept())
      try {
        val start = p.receive()
        val mappings = Json.arr((1 to count).map(ch => Json.obj("channel" -> Json.fromInt(ch), "direction" -> Json.string("output"), "streamToken" -> Json.string(s"output-$ch"))).toVector)
        val pongs = Future { scala.concurrent.blocking { try p.receive() catch { case _: java.io.IOException => () } } }
        p.send(message("invocationAccepted", "attemptId" -> field(start, "attemptId"), "idempotencyKey" -> field(start, "idempotencyKey"),
          "sessionToken" -> Json.string("session"), "mappings" -> mappings))
        val element = Json.obj("kind" -> Json.string("string"), "value" -> Json.string("x" * chunkSize))
        val outcome = Json.obj("kind" -> Json.string("failed"), "error" -> Json.obj("kind" -> Json.string("failed"),
          "value" -> Json.obj("graph" -> failureGraph, "value" -> Json.obj("kind" -> Json.string("list"), "value" -> Json.obj("elements" -> Json.arr(Vector.fill(elementCount)(element)))))))
        def terminal(ch: Int): Unit = p.send(message("stream-end", "channel" -> Json.fromInt(ch), "direction" -> Json.string("output"),
          "sequence" -> Json.string("0"), "outcome" -> outcome))
        if (mode.endsWith("limit")) {
          (1 to count).foreach(terminal)
          Await.result(released.future, 30.seconds)
        } else {
          terminal(1)
          terminal(2)
          val refs = (1 to count).map(ch => Json.obj("kind" -> Json.string("stream"), "value" -> Json.obj("streamToken" -> Json.string(s"output-$ch")))).toVector
          p.send(message("invocationResult", "result" -> Json.obj("kind" -> Json.string("value"), "graph" -> graph,
            "value" -> Json.obj("kind" -> Json.string("tuple"), "value" -> Json.obj("elements" -> Json.arr(refs)))), "mappings" -> mappings))
          Await.result(released.future, 30.seconds)
          terminal(3)
          p.send(message("invocationFinished", "outcome" -> Json.obj("kind" -> Json.string("success"))))
        }
      } finally { p.close(); server.close() }
    }
    val empty = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"tuple","value":{"elements":[]}}}""")
    val resolved = ResolvedAgent(Configuration(GolemServer.Custom(s"http://127.0.0.1:${server.getLocalPort}", "token"), "app", "env", global),
      "RecoveryAgent", SchemaValue.TupleValue(Nil), None, Nil, None)
    try {
      val invocation = Bridge.invokeStreamingAgent(resolved, "run", () => SchemaValue.TupleValue(Nil), empty, empty,
        Some(PublicValueCodec.fromSchemaGraphJson(graph.render)), Nil)
      val checkedInvocation = Future.firstCompletedOf(List(invocation, peer.flatMap(_ => Promise[AgentInvocationResult]().future)))
      if (mode.endsWith("limit")) {
        val error = intercept[BridgeException](Await.result(checkedInvocation, 30.seconds))
        assert(error.getMessage.contains(if (mode == "stream-limit") "output stream queue" else "session output queue"))
        released.success(())
      } else {
        val result = Await.result(checkedInvocation, 30.seconds)
        val values = result.result.get.asInstanceOf[SchemaValue.TupleValue].elements
        val consumers = values.map(value => Await.result(StreamSession.output(value, SchemaValueCodec.asString, "json",
          PublicValueCodec.fromSchemaGraphJson(stringGraph.render)).consume(), 5.seconds))
        def consumeFailure(index: Int): Unit = {
          val error = intercept[AgentStreamEnded](Await.result(consumers(index).pull(), 30.seconds))
          val failed = error.outcome.asInstanceOf[StreamOutcome.Failed].error.asInstanceOf[StreamError.Failed]
          assertEquals(failed.graph, failureGraph)
          val values = failed.value.asInstanceOf[SchemaValue.ListValue].elements
          assertEquals(values.size, elementCount)
          values.foreach { element =>
            val value = element.asInstanceOf[SchemaValue.StringValue].value
            assertEquals(value.length, chunkSize)
            assert(value.forall(_ == 'x'))
          }
        }
        if (mode == "dispose") Await.result(consumers.head.drop(), 5.seconds) else consumeFailure(0)
        released.success(())
        consumeFailure(2)
        consumeFailure(1)
      }
      Await.result(peer, 30.seconds)
    } finally { released.trySuccess(()); server.close() }
  }

  for ((scenario, code, retryable) <- List(
    ("initial", "invalid-attachment-state", true),
    ("accepted-start", "invalid-attachment-state", true),
    ("other-resume", "permission-denied", false),
    ("takeover", "invalid-attachment-state", false),
    ("takeover-retryable", "invalid-attachment-state", true),
    ("stale-start", "invalid-attachment-state", false)
  )) test(s"$scenario rejection is terminal") {
    val server = new ServerSocket(0)
    val failed = Promise[Unit]()
    val peer = Future {
      var sockets = List.empty[Peer]
      def connect(): Peer = { val p = new Peer(server.accept()); sockets ::= p; p }
      try {
        val first = connect()
        val start = first.receive()
        def accept(p: Peer, descriptor: Json): Unit = p.send(message("invocationAccepted",
          "attemptId" -> field(descriptor, "attemptId"), "idempotencyKey" -> field(start, "idempotencyKey"),
          "sessionToken" -> Json.string("stable-session"), "mappings" -> Json.arr(Vector.empty)))
        def reject(p: Peer, descriptor: Json, rejectionCode: String, canRetry: Boolean): Unit =
          p.send(message("invocationRejected", "attemptId" -> field(descriptor, "attemptId"),
            "code" -> Json.string(rejectionCode), "message" -> Json.string("rejected"), "retryable" -> Json.bool(canRetry)))
        if (scenario == "initial") reject(first, start, code, retryable)
        else {
          accept(first, start)
          if (scenario == "accepted-start") reject(first, start, code, retryable)
          else {
            first.close()
            val resumePeer = connect()
            val resume = resumePeer.receive()
            assertEquals(text(resume, "operation"), "resume")
            if (scenario.startsWith("takeover")) {
              reject(resumePeer, resume, "invalid-attachment-state", false)
              val takeoverPeer = connect()
              val takeover = takeoverPeer.receive()
              assertEquals(text(takeover, "operation"), "takeover")
              reject(takeoverPeer, takeover, code, retryable)
            } else reject(resumePeer, if (scenario == "stale-start") start else resume, code, retryable)
          }
        }
        Await.result(failed.future, 10.seconds)
        server.setSoTimeout(500)
        intercept[java.net.SocketTimeoutException] { server.accept() }
      } finally { sockets.foreach(_.close()); server.close() }
    }
    val empty = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"tuple","value":{"elements":[]}}}""")
    val resolved = ResolvedAgent(Configuration(GolemServer.Custom(s"http://127.0.0.1:${server.getLocalPort}", "token"), "app", "env", global),
      "RecoveryAgent", SchemaValue.TupleValue(Nil), None, Nil, None)
    try {
      val error = intercept[BridgeException] {
        Await.result(Bridge.invokeStreamingAgent(resolved, "run", () => SchemaValue.TupleValue(Nil), empty, empty, None, Nil), 15.seconds)
      }
      assert(error.getMessage.contains(if (scenario == "stale-start") "pending operation" else code))
      failed.success(())
      Await.result(peer, 10.seconds)
    } finally server.close()
  }
}
