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
    private def sendFrame(op: Int, bytes: Array[Byte]): Unit = {
      out.write(128 | op)
      if (bytes.length < 126) out.write(bytes.length)
      else { out.write(126); out.write(bytes.length >> 8); out.write(bytes.length & 255) }
      out.write(bytes); out.flush()
    }
    def send(value: Json): Unit = sendFrame(1, value.render.getBytes(UTF_8))
    def receive(): Json = {
      val op = in.read() & 15
      val size = in.read() & 127
      val length = if (size == 126) (in.read() << 8) | in.read() else size
      val mask = in.readNBytes(4)
      val bytes = in.readNBytes(length)
      val decoded = bytes.indices.map(i => (bytes(i) ^ mask(i % 4)).toByte).toArray
      if (op == 9) { sendFrame(10, decoded); receive() }
      else Json.parse(new String(decoded, UTF_8)).toOption.get
    }
    def close(): Unit = socket.close()
  }
  private def field(value: Json, name: String): Json = Json.requireField(value, name).toOption.get
  private def text(value: Json, name: String): String = Json.asString(field(value, name)).toOption.get
  private def message(kind: String, fields: (String, Json)*): Json =
    Json.parse(StreamSessionProtocol.message(kind, fields.toVector)).toOption.get

  for (rejectAfterTakeover <- List(false, true)) test(s"resume switches to takeover and retries its uncertain descriptor (retry after acceptance: $rejectAfterTakeover)") {
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
        finalPeer.send(message("outputStreamEnd", "channel" -> Json.fromLong(1), "sequence" -> Json.string("2"),
          "cursorToken" -> Json.string("terminal-cursor"), "outcome" -> Json.obj("kind" -> Json.string("ok"))))
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
      assertEquals(Await.result(consumer.pull(), 5.seconds), AgentStreamStep.End)
    } finally server.close()
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
