/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.websocket

import golem.FutureInterop
import scala.concurrent.{ExecutionContext, Future}
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.scalajs.js.annotation.JSImport
import scala.scalajs.js.typedarray.Uint8Array

/**
 * What a connection does after runtime reconstruction (suspension, restart or
 * crash), when its physical session is gone.
 */
sealed trait ReconstructionPolicy extends Product with Serializable

object ReconstructionPolicy {

  /**
   * Opens a replacement connection with the same URL and headers on the first
   * live access. Choose it only when the peer accepts a new session in place of
   * the lost one.
   */
  case object ReconnectAutomatically extends ReconstructionPolicy

  /**
   * The first live access returns [[WebsocketError.SessionLost]], and the
   * connection stays lost. This is the host default, used when `connect` gets
   * no policy.
   */
  case object ReportConnectionLoss extends ReconstructionPolicy
}

/** A WebSocket message: text or binary. */
sealed trait WebsocketMessage extends Product with Serializable

object WebsocketMessage {
  final case class Text(value: String)         extends WebsocketMessage
  final case class Binary(value: Vector[Byte]) extends WebsocketMessage
}

final case class CloseInfo(code: Int, reason: String)

/** The `golem:websocket/client` error variant. */
sealed trait WebsocketError extends Product with Serializable

object WebsocketError {
  final case class ConnectionFailure(message: String) extends WebsocketError
  final case class SendFailure(message: String)       extends WebsocketError
  final case class ReceiveFailure(message: String)    extends WebsocketError
  final case class ProtocolError(message: String)     extends WebsocketError
  final case class Closed(info: Option[CloseInfo])    extends WebsocketError
  final case class Other(message: String)             extends WebsocketError

  /**
   * The physical session did not survive runtime reconstruction of a connection
   * that reports connection loss. An earlier send may still have reached the
   * peer.
   */
  case object SessionLost extends WebsocketError
}

/** An open outbound WebSocket connection. */
final class WebsocketConnection private (raw: js.Dynamic) {
  import WebsocketConnection._

  def send(message: WebsocketMessage): Either[WebsocketError, Unit] =
    attempt(raw.send(toHost(message))).map(_ => ())

  /** Receives the next message, waiting until one is available. */
  def receive(): Future[Either[WebsocketError, WebsocketMessage]] =
    attemptAsync(raw.receive()).map(_.map(fromHost))(ExecutionContext.parasitic)

  /**
   * Receives the next message, waiting up to `timeoutMs` milliseconds. `None`
   * means the timeout expired first.
   */
  def receiveWithTimeout(timeoutMs: Long): Future[Either[WebsocketError, Option[WebsocketMessage]]] =
    attemptAsync(raw.receiveWithTimeout(js.BigInt(timeoutMs.toString)))
      .map(_.map(message => message.asInstanceOf[js.UndefOr[js.Dynamic]].toOption.map(fromHost)))(
        ExecutionContext.parasitic
      )

  /** Sends a close frame with an optional code and reason. */
  def close(code: Option[Int] = None, reason: Option[String] = None): Either[WebsocketError, Unit] =
    attempt(raw.close(code.orUndefined, reason.orUndefined)).map(_ => ())

  /** Releases the resource locally without a close frame. */
  def release(): Unit = {
    val symbol = js.Dynamic.global.Symbol.selectDynamic("dispose")
    js.Dynamic.global.Reflect.get(raw, symbol).applyDynamic("call")(raw)
    ()
  }
}

object WebsocketConnection {

  /**
   * Connects to `url` (`ws://` or `wss://`).
   *
   * Without `reconstructionPolicy`, no policy is sent and the host default
   * [[ReconstructionPolicy.ReportConnectionLoss]] applies: after runtime
   * reconstruction the next operation returns [[WebsocketError.SessionLost]]
   * and no replacement connection is opened.
   */
  def connect(
    url: String,
    headers: Seq[(String, String)] = Nil,
    reconstructionPolicy: Option[ReconstructionPolicy] = None
  ): Either[WebsocketError, WebsocketConnection] =
    attempt(
      Host.WebsocketConnection.connect(
        url,
        if (headers.isEmpty) js.undefined else headers.map { case (k, v) => js.Array(k, v) }.toJSArray,
        reconstructionPolicy.map {
          case ReconstructionPolicy.ReconnectAutomatically => "reconnect-automatically"
          case ReconstructionPolicy.ReportConnectionLoss   => "report-connection-loss"
        }.orUndefined
      )
    ).map(new WebsocketConnection(_))

  private def attempt(operation: => js.Dynamic): Either[WebsocketError, js.Dynamic] =
    try Right(operation)
    catch {
      case exception @ js.JavaScriptException(thrown) => Left(hostError(thrown).getOrElse(throw exception))
    }

  private def attemptAsync(operation: => js.Dynamic): Future[Either[WebsocketError, js.Dynamic]] =
    attempt(operation) match {
      case Left(error)    => Future.successful(Left(error))
      case Right(promise) =>
        FutureInterop
          .fromPromise(promise.asInstanceOf[js.Promise[js.Dynamic]])
          .map(Right(_): Either[WebsocketError, js.Dynamic])(ExecutionContext.parasitic)
          .recover { case exception @ js.JavaScriptException(thrown) =>
            Left(hostError(thrown).getOrElse(throw exception))
          }(ExecutionContext.parasitic)
    }

  /**
   * The host throws the WIT error variant, or an `Error` carrying it as
   * `payload` or `cause`.
   */
  private def hostError(thrown: Any): Option[WebsocketError] = {
    def tagged(value: Any): Option[js.Dynamic] =
      if (value != null && js.typeOf(value) == "object") {
        val candidate = value.asInstanceOf[js.Dynamic]
        if (js.typeOf(candidate.tag) == "string") Some(candidate)
        else tagged(candidate.payload).orElse(tagged(candidate.cause))
      } else None
    def text(error: js.Dynamic): String = error.selectDynamic("val").asInstanceOf[String]
    tagged(thrown).flatMap { error =>
      error.tag.asInstanceOf[String] match {
        case "connection-failure" => Some(WebsocketError.ConnectionFailure(text(error)))
        case "send-failure"       => Some(WebsocketError.SendFailure(text(error)))
        case "receive-failure"    => Some(WebsocketError.ReceiveFailure(text(error)))
        case "protocol-error"     => Some(WebsocketError.ProtocolError(text(error)))
        case "other"              => Some(WebsocketError.Other(text(error)))
        case "session-lost"       => Some(WebsocketError.SessionLost)
        case "closed"             =>
          Some(
            WebsocketError.Closed(
              error
                .selectDynamic("val")
                .asInstanceOf[js.UndefOr[js.Dynamic]]
                .toOption
                .map(info => CloseInfo(info.code.asInstanceOf[Int], info.reason.asInstanceOf[String]))
            )
          )
        case _ => None
      }
    }
  }

  private def toHost(message: WebsocketMessage): js.Dynamic = message match {
    case WebsocketMessage.Text(value)   => js.Dynamic.literal(tag = "text", `val` = value)
    case WebsocketMessage.Binary(value) =>
      val bytes = new Uint8Array(value.size)
      value.indices.foreach(i => bytes(i) = (value(i) & 0xff).toShort)
      js.Dynamic.literal(tag = "binary", `val` = bytes)
  }

  private def fromHost(message: js.Dynamic): WebsocketMessage =
    message.tag.asInstanceOf[String] match {
      case "text" => WebsocketMessage.Text(message.selectDynamic("val").asInstanceOf[String])
      case _      =>
        val bytes = message.selectDynamic("val").asInstanceOf[Uint8Array]
        WebsocketMessage.Binary(Vector.tabulate(bytes.length)(i => bytes(i).toByte))
    }

  @js.native
  @JSImport("golem:websocket/client@1.5.0", JSImport.Namespace)
  private object Host extends js.Object {
    val WebsocketConnection: js.Dynamic = js.native
  }
}
