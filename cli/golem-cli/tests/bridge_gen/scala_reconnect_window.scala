package golem.bridge.runtime

import golem.bridge.client.streamer_agent.StreamerAgentClient
import golem.bridge.runtime.json.Json
import java.net.{ServerSocket, Socket}
import java.nio.charset.StandardCharsets.UTF_8
import java.security.MessageDigest
import java.util.Base64
import java.util.concurrent.atomic.AtomicInteger
import scala.concurrent.{Await, Future}
import scala.concurrent.ExecutionContext.Implicits.global
import scala.concurrent.duration.*

class StreamRuntimeTest extends munit.FunSuite {
  // Generated streaming methods keep their plain form and gain a cancelable one.
  def generatedCancelable(agent: StreamerAgentClient.StreamerAgentRemote): CancellationToken = agent.numbers.cancelable()._2

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
    // Ends the connection with a close frame, so the client sees the loss at once.
    def hangUp(): Unit = { sendFrame(8, Array(3.toByte, 232.toByte)); socket.close() }
  }

  private def refuse(server: ServerSocket, status: Int): Unit = {
    val socket = server.accept()
    try {
      val in = socket.getInputStream
      val request = new StringBuilder
      while (!request.toString.endsWith("\r\n\r\n")) request.append(in.read().toChar)
      socket.getOutputStream.write(s"HTTP/1.1 $status Refused\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".getBytes(UTF_8))
      socket.getOutputStream.flush()
    } finally socket.close()
  }

  private def field(value: Json, name: String): Json = Json.requireField(value, name).toOption.get
  private def text(value: Json, name: String): String = Json.asString(field(value, name)).toOption.get
  private def message(kind: String, fields: (String, Json)*): Json =
    Json.parse(StreamSessionProtocol.message(kind, fields.toVector)).toOption.get
  private def accepted(p: Peer, descriptor: Json, key: Json): Unit = p.send(message("invocationAccepted",
    "attemptId" -> field(descriptor, "attemptId"), "idempotencyKey" -> key,
    "sessionToken" -> Json.string("stable-session"), "mappings" -> Json.arr(Vector.empty)))
  private def finish(p: Peer): Unit = {
    p.send(message("invocationResult", "result" -> Json.obj("kind" -> Json.string("none")), "mappings" -> Json.arr(Vector.empty)))
    p.send(message("invocationFinished", "outcome" -> Json.obj("kind" -> Json.string("success"))))
  }
  private def rejection(descriptor: Json): Json = message("invocationRejected", "attemptId" -> field(descriptor, "attemptId"),
    "code" -> Json.string("routing-miss"), "message" -> Json.string("moving"), "retryable" -> Json.bool(true))

  private val empty = PublicValueCodec.fromSchemaGraphJson("""{"root":{"kind":"tuple","value":{"elements":[]}}}""")
  private def call(port: Int, window: Duration): (Future[AgentInvocationResult], CancellationToken) = {
    val resolved = ResolvedAgent(Configuration(GolemServer.Custom(s"http://127.0.0.1:$port", "token"), "app", "env", global, window),
      "StreamerAgent", SchemaValue.TupleValue(Nil), None, Nil, None)
    Bridge.invokeStreamingAgentCancelable(resolved, "run", () => SchemaValue.TupleValue(Nil), empty, empty, None, Nil)
  }
  // Runs body with a shorter time an accepted connection must stay up to count as progress.
  private def stableAfter[A](millis: Long)(body: => A): A = {
    val before = StreamSession.stableConnectionMillis
    StreamSession.stableConnectionMillis = millis
    try body finally StreamSession.stableConnectionMillis = before
  }
  private def closedPort(): Int = { val server = new ServerSocket(0); val port = server.getLocalPort; server.close(); port }

  test("a start against a closed port is unreachable after the window") {
    val port = closedPort()
    val started = System.nanoTime()
    val error = intercept[BridgeUnreachableException](Await.result(call(port, 300.millis)._1, 10.seconds))
    val elapsed = (System.nanoTime() - started).nanos
    assert(error.endpoint.contains(s":$port/"), error.endpoint)
    assertEquals(error.reconnectWindow, 300.millis)
    assert(error.lastCause != null)
    assert(error.isInstanceOf[BridgeException])
    assert(elapsed >= 300.millis && elapsed < 5.seconds, elapsed.toString)
  }

  test("an upgrade that never completes is abandoned within the window") {
    val server = new ServerSocket(0)
    val held = new java.util.concurrent.ConcurrentLinkedQueue[Socket]()
    val acceptor = new Thread(() => try while (true) held.add(server.accept()) catch { case _: Throwable => () })
    acceptor.setDaemon(true)
    acceptor.start()
    try {
      val started = System.nanoTime()
      val error = intercept[BridgeUnreachableException](Await.result(call(server.getLocalPort, 1.second)._1, 10.seconds))
      assert((System.nanoTime() - started).nanos < 5.seconds)
      assert(error.lastCause.isInstanceOf[java.net.http.HttpTimeoutException], String.valueOf(error.lastCause))
    } finally { server.close(); held.forEach(_.close()) }
  }

  test("429 and 5xx on the upgrade are retried") {
    val server = new ServerSocket(0)
    val peer = Future {
      try {
        refuse(server, 503); refuse(server, 429); refuse(server, 502)
        val p = new Peer(server.accept())
        val start = p.receive()
        accepted(p, start, field(start, "idempotencyKey"))
        finish(p)
        p
      } finally server.close()
    }
    Await.result(call(server.getLocalPort, 10.seconds)._1, 15.seconds)
    Await.result(peer, 5.seconds).close()
  }

  test("another upgrade status fails at once") {
    val server = new ServerSocket(0)
    val peer = Future(try refuse(server, 403) finally ())
    try {
      val error = intercept[java.net.http.WebSocketHandshakeException](Await.result(call(server.getLocalPort, Duration.Inf)._1, 10.seconds))
      assertEquals(error.getResponse.statusCode(), 403)
      Await.result(peer, 5.seconds)
      server.setSoTimeout(300)
      intercept[java.net.SocketTimeoutException](server.accept())
    } finally server.close()
  }

  test("no reconnect limit keeps retrying") {
    val server = new ServerSocket(0)
    val peer = Future {
      try {
        (1 to 5).foreach(_ => refuse(server, 503))
        val p = new Peer(server.accept())
        val start = p.receive()
        accepted(p, start, field(start, "idempotencyKey"))
        finish(p)
        p
      } finally server.close()
    }
    Await.result(call(server.getLocalPort, Duration.Inf)._1, 15.seconds)
    Await.result(peer, 5.seconds).close()
  }

  test("a retryable rejection before acceptance starts a new attempt") {
    val server = new ServerSocket(0)
    val peer = Future {
      try {
        val first = new Peer(server.accept())
        val start = first.receive()
        first.send(rejection(start))
        val second = new Peer(server.accept())
        val retry = second.receive()
        assertEquals(text(retry, "type"), "invocationStart")
        assertNotEquals(field(retry, "attemptId").render, field(start, "attemptId").render)
        assertEquals(field(retry, "idempotencyKey").render, field(start, "idempotencyKey").render)
        accepted(second, retry, field(start, "idempotencyKey"))
        finish(second)
        List(first, second)
      } finally server.close()
    }
    Await.result(call(server.getLocalPort, 10.seconds)._1, 15.seconds)
    Await.result(peer, 5.seconds).foreach(_.close())
  }

  test("retryable rejections past the window are unreachable") {
    val server = new ServerSocket(0)
    val peers = new java.util.concurrent.ConcurrentLinkedQueue[Peer]()
    val acceptor = new Thread(() => try while (true) {
      val p = new Peer(server.accept())
      peers.add(p)
      p.send(rejection(p.receive()))
    } catch { case _: Throwable => () })
    acceptor.setDaemon(true)
    acceptor.start()
    try {
      val error = intercept[BridgeUnreachableException](Await.result(call(server.getLocalPort, 400.millis)._1, 10.seconds))
      assert(error.lastCause.getMessage.contains("routing-miss"), error.lastCause.getMessage)
      assert(peers.size() >= 2)
    } finally { server.close(); peers.forEach(_.close()) }
  }

  test("a server that drops every start is unreachable after the window, with backoff") {
    val server = new ServerSocket(0)
    val connections = new AtomicInteger(0)
    val acceptor = new Thread(() => try while (true) {
      val p = new Peer(server.accept())
      connections.incrementAndGet()
      p.receive()
      p.close()
    } catch { case _: Throwable => () })
    acceptor.setDaemon(true)
    acceptor.start()
    try {
      intercept[BridgeUnreachableException](Await.result(call(server.getLocalPort, 400.millis)._1, 10.seconds))
      val count = connections.get()
      assert(count >= 2 && count <= 8, s"connected $count times")
    } finally server.close()
  }

  test("a start the server never answers is unreachable after the window") {
    val server = new ServerSocket(0)
    val connections = new AtomicInteger(0)
    val acceptor = new Thread(() => try while (true) {
      val p = new Peer(server.accept())
      connections.incrementAndGet()
      p.receive()
      // Answers pings and nothing else.
      while (true) p.receive()
    } catch { case _: Throwable => () })
    acceptor.setDaemon(true)
    acceptor.start()
    try {
      val started = System.nanoTime()
      val error = intercept[BridgeUnreachableException](Await.result(call(server.getLocalPort, 600.millis)._1, 10.seconds))
      val elapsed = (System.nanoTime() - started).nanos
      assertEquals(error.reconnectWindow, 600.millis)
      assert(error.lastCause.getMessage.contains("answered neither the start nor the resume"), String.valueOf(error.lastCause))
      assert(elapsed >= 600.millis && elapsed < 5.seconds, elapsed.toString)
      assertEquals(connections.get(), 1)
    } finally server.close()
  }

  test("the window starts again when an accepted connection is lost") {
    val server = new ServerSocket(0)
    val peer = Future {
      try {
        val first = new Peer(server.accept())
        val start = first.receive()
        val key = field(start, "idempotencyKey")
        accepted(first, start, key)
        Thread.sleep(500)
        first.close()
        val second = new Peer(server.accept())
        val resume = second.receive()
        assertEquals(text(resume, "operation"), "resume")
        accepted(second, resume, key)
        finish(second)
        second
      } finally server.close()
    }
    stableAfter(200)(Await.result(call(server.getLocalPort, 300.millis)._1, 15.seconds))
    Await.result(peer, 5.seconds).close()
  }

  test("an accepted connection dropped at once is unreachable after the window, with backoff") {
    val server = new ServerSocket(0)
    val connected = new java.util.concurrent.ConcurrentLinkedQueue[java.lang.Long]()
    @volatile var key: Json = null
    val acceptor = new Thread(() => try while (true) {
      val p = new Peer(server.accept())
      connected.add(System.nanoTime())
      val operation = p.receive()
      if (key == null) key = field(operation, "idempotencyKey")
      accepted(p, operation, key)
      p.hangUp()
    } catch { case _: Throwable => () })
    acceptor.setDaemon(true)
    acceptor.start()
    try {
      val started = System.nanoTime()
      stableAfter(200) {
        intercept[BridgeUnreachableException](Await.result(call(server.getLocalPort, 400.millis)._1, 10.seconds))
      }
      val elapsed = (System.nanoTime() - started).nanos
      assert(elapsed >= 400.millis && elapsed < 5.seconds, elapsed.toString)
      val times = connected.toArray(Array.empty[java.lang.Long]).map(_.longValue)
      assert(times.length >= 3 && times.length <= 6, s"connected ${times.length} times")
      val lastWait = (times(times.length - 1) - times(times.length - 2)).nanos
      assert(lastWait >= 150.millis, s"the last reconnect waited only $lastWait")
    } finally server.close()
  }

  test("stable connections keep a call alive past the window") {
    val server = new ServerSocket(0)
    val peer = Future {
      try {
        val first = new Peer(server.accept())
        val start = first.receive()
        val key = field(start, "idempotencyKey")
        accepted(first, start, key)
        Thread.sleep(250)
        var lost = System.nanoTime()
        first.close()
        val waits = List.newBuilder[FiniteDuration]
        (1 to 3).foreach { _ =>
          val p = new Peer(server.accept())
          waits += (System.nanoTime() - lost).nanos
          accepted(p, p.receive(), key)
          Thread.sleep(250)
          lost = System.nanoTime()
          p.close()
        }
        val last = new Peer(server.accept())
        waits += (System.nanoTime() - lost).nanos
        accepted(last, last.receive(), key)
        finish(last)
        (last, waits.result())
      } finally server.close()
    }
    val started = System.nanoTime()
    stableAfter(100)(Await.result(call(server.getLocalPort, 400.millis)._1, 15.seconds))
    assert((System.nanoTime() - started).nanos >= 1.second)
    val (last, waits) = Await.result(peer, 5.seconds)
    last.close()
    waits.foreach(wait => assert(wait < 100.millis, s"a reconnect after a stable connection waited $wait"))
  }

  test("cancelling a start that cannot connect fails it with BridgeCancelledException") {
    val server = new ServerSocket(0)
    val refused = new AtomicInteger(0)
    val refuser = new Thread(() => try while (true) { refuse(server, 503); refused.incrementAndGet() } catch { case _: Throwable => () })
    refuser.setDaemon(true)
    refuser.start()
    try {
      val (invocation, cancellation) = call(server.getLocalPort, Duration.Inf)
      while (refused.get() < 2) Thread.sleep(10)
      cancellation.cancel()
      intercept[BridgeCancelledException](Await.result(invocation, 5.seconds))
      Thread.sleep(200)
      val afterCancel = refused.get()
      Thread.sleep(500)
      assertEquals(refused.get(), afterCancel)
    } finally server.close()
  }

  test("cancelling an accepted call closes its session") {
    val server = new ServerSocket(0)
    val peer = Future {
      try {
        val p = new Peer(server.accept())
        val start = p.receive()
        accepted(p, start, field(start, "idempotencyKey"))
        p
      } finally ()
    }
    try {
      val (invocation, cancellation) = call(server.getLocalPort, Duration.Inf)
      val p = Await.result(peer, 10.seconds)
      Thread.sleep(100)
      cancellation.cancel()
      intercept[BridgeCancelledException](Await.result(invocation, 5.seconds))
      server.setSoTimeout(500)
      intercept[java.net.SocketTimeoutException](server.accept())
      p.close()
    } finally server.close()
  }
}
