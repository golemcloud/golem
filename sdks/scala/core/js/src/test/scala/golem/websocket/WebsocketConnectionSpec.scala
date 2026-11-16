/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.websocket

import scala.scalajs.js
import zio.ZIO
import zio.test._

object WebsocketConnectionSpec extends ZIOSpecDefault {
  private final class Recorded {
    var connects: Vector[(String, js.Any, js.Any)] = Vector.empty
  }

  private def install(connection: js.Object): Recorded = {
    val recorded = new Recorded
    js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(
      js.Dynamic.literal(websocketConnect =
        (
          (url: String, headers: js.Any, policy: js.Any) => {
            recorded.connects :+= ((url, headers, policy))
            connection
          }
        ): js.Function3[String, js.Any, js.Any, js.Object]
      )
    )
    recorded
  }

  private def throwing(error: js.Any): js.Function0[js.Any] = () => throw js.JavaScriptException(error)

  override def spec = suite("WebsocketConnectionSpec")(
    test("connect without a policy sends none, so the host default applies; a policy is sent as given") {
      val recorded = install(js.Dynamic.literal())
      WebsocketConnection.connect("wss://peer")
      WebsocketConnection.connect(
        "wss://peer",
        Seq("authorization" -> "token"),
        Some(ReconstructionPolicy.ReconnectAutomatically)
      )
      WebsocketConnection.connect("wss://peer", reconstructionPolicy = Some(ReconstructionPolicy.ReportConnectionLoss))
      val (absent, automatic, report) = (recorded.connects(0), recorded.connects(1), recorded.connects(2))
      assertTrue(
        recorded.connects.size == 3,
        js.isUndefined(absent._2),
        js.isUndefined(absent._3),
        js.JSON.stringify(automatic._2) == """[["authorization","token"]]""",
        automatic._3.asInstanceOf[String] == "reconnect-automatically",
        report._3.asInstanceOf[String] == "report-connection-loss"
      )
    },
    test("session-lost is its own typed error on every operation, apart from ordinary failures") {
      val loss = js.Dynamic.literal(tag = "session-lost")
      install(
        js.Dynamic.literal(
          send = throwing(loss),
          close = throwing(js.Dynamic.newInstance(js.Dynamic.global.Error)("host", js.Dynamic.literal(cause = loss))),
          receive = (() => js.Promise.reject(loss)): js.Function0[js.Any],
          receiveWithTimeout = ((_: js.Any) => js.Promise.reject(loss)): js.Function1[js.Any, js.Any]
        )
      )
      val connection = WebsocketConnection.connect("wss://peer").toOption.get
      ZIO.fromFuture { implicit ec =>
        for {
          received <- connection.receive()
          timed    <- connection.receiveWithTimeout(5)
        } yield (received, timed)
      }.map { case (received, timed) =>
        assertTrue(
          connection.send(WebsocketMessage.Text("hello")) == Left(WebsocketError.SessionLost),
          connection.close() == Left(WebsocketError.SessionLost),
          received == Left(WebsocketError.SessionLost),
          timed == Left(WebsocketError.SessionLost)
        )
      }
    },
    test("connection and close failures keep their own cases") {
      install(js.Dynamic.literal())
      js.Dynamic.global.globalThis.__golemScalaTestHost.updateDynamic("websocketConnect")(
        throwing(js.Dynamic.literal(tag = "connection-failure", `val` = "refused"))
      )
      val refused = WebsocketConnection.connect("wss://peer")
      install(
        js.Dynamic.literal(
          receive = (
            () =>
              js.Promise.reject(
                js.Dynamic.literal(tag = "closed", `val` = js.Dynamic.literal(code = 1001, reason = "bye"))
              )
          ): js.Function0[js.Any]
        )
      )
      ZIO.fromFuture(_ => WebsocketConnection.connect("wss://peer").toOption.get.receive()).map { closed =>
        assertTrue(
          refused == Left(WebsocketError.ConnectionFailure("refused")),
          closed == Left(WebsocketError.Closed(Some(CloseInfo(1001, "bye"))))
        )
      }
    },
    test("messages round-trip text and binary frames") {
      var sent = Vector.empty[js.Dynamic]
      install(
        js.Dynamic.literal(
          send = ((message: js.Dynamic) => { sent :+= message; () }): js.Function1[js.Dynamic, Unit],
          receiveWithTimeout = (
            (_: js.Any) =>
              js.Promise.resolve[js.Any](
                js.Dynamic.literal(tag = "binary", `val` = new js.typedarray.Uint8Array(js.Array[Short](1, 255)))
              )
          ): js.Function1[js.Any, js.Any]
        )
      )
      val connection = WebsocketConnection.connect("wss://peer").toOption.get
      connection.send(WebsocketMessage.Text("hi"))
      connection.send(WebsocketMessage.Binary(Vector(1.toByte, -1.toByte)))
      ZIO.fromFuture(_ => connection.receiveWithTimeout(5)).map { received =>
        assertTrue(
          sent(0).tag.asInstanceOf[String] == "text",
          sent(0).selectDynamic("val").asInstanceOf[String] == "hi",
          sent(1).selectDynamic("val").asInstanceOf[js.typedarray.Uint8Array](1) == 255,
          received == Right(Some(WebsocketMessage.Binary(Vector(1.toByte, -1.toByte))))
        )
      }
    }
  )
}
