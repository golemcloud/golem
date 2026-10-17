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

package golem.runtime.tool.client

import golem.FutureInterop
import golem.host.SchemaWireInterop
import golem.host.js.tool.JsInvocationResult
import golem.host.js.schema.JsTypedSchemaValue
import golem.runtime.tool.JsToolInputStream
import golem.runtime.tool.host.ToolHostApi
import golem.schema.{AgentStreamOwnership, TypedSchemaValue}
import golem.schema.wire.{SchemaWire, WitTypedSchemaValue}
import golem.tool._

import scala.concurrent.{Future, Promise}
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.util.{Failure, Success, Try}
import zio.blocks.async.*
import zio.blocks.streams.internal.StreamError

/**
 * Entry point generated typed tool clients use to obtain the RPC transport of
 * one remote tool.
 */
object ToolRpcClient {

  /** A transport bound to one remote tool name. */
  def transport(toolName: String): ToolRpcTransport =
    tryTransport(toolName) match {
      case Right(transport) => transport
      case Left(failure)    => throw new ToolRpcConstructionException(failure)
    }

  def tryTransport(toolName: String): Either[ToolRpcFailure, ToolRpcTransport] =
    try Right(new JsToolRpcTransport(ToolHostApi.RawToolRpc.create(toolName)))
    catch {
      case js.JavaScriptException(error)      => Left(ToolHostApi.decodeRpcFailure(error))
      case scala.util.control.NonFatal(error) =>
        Left(ToolRpcFailure.ProtocolError(String.valueOf(error.getMessage)))
    }

  def trigger(
    toolName: String,
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream]
  ): Either[ToolRpcFailure, Unit] =
    try new JsToolRpcTransport(ToolHostApi.RawToolRpc.create(toolName)).trigger(commandPath, input, stdin)
    catch {
      case js.JavaScriptException(error)      => Left(ToolHostApi.decodeRpcFailure(error))
      case scala.util.control.NonFatal(error) =>
        Left(ToolRpcFailure.ProtocolError(String.valueOf(error.getMessage)))
    }

  def wireTransport(toolName: String): WireToolRpcTransport =
    try new JsWireToolRpcTransport(ToolHostApi.RawToolRpc.create(toolName))
    catch {
      case js.JavaScriptException(error) =>
        throw new ToolRpcConstructionException(ToolHostApi.decodeRpcFailure(error))
      case scala.util.control.NonFatal(error) =>
        throw new ToolRpcConstructionException(
          ToolRpcFailure.ProtocolError(String.valueOf(error.getMessage))
        )
    }
}

private[golem] final class JsWireToolRpcTransport(
  rpc: ToolHostApi.RawToolRpc,
  encode: WitTypedSchemaValue => Future[JsTypedSchemaValue] = SchemaWireInterop.typedToJsAsync,
  createOutput: () => (ToolHostApi.RawToolOutput, ToolHostApi.RawByteStream) = () => ToolHostApi.createOutput()
) extends WireToolRpcTransport {
  private implicit val ec: scala.concurrent.ExecutionContext = ToolInvokerRuntime.executionContext

  def start(
    commandPath: List[String],
    input: WitTypedSchemaValue,
    stdin: Option[ToolInputStream],
    stdout: Boolean,
    stderr: Boolean
  ): Either[WireToolRpcFailure, WireToolRpcStarted] = {
    val lease = new ToolResultObserver[Either[WireToolRpcFailure, WireToolInvokeResult]]
    try {
      val stdoutEndpoints = if (stdout) Some(createOutput()) else None
      val stderrEndpoints = if (stderr) Some(createOutput()) else None
      val stdoutStream    = stdoutEndpoints.map(e => new JsToolInputStream(e._2))
      val stderrStream    = stderrEndpoints.map(e => new JsToolInputStream(e._2))
      val pumpTransport   = new JsToolRpcTransport(rpc)
      val encoded         = encode(input).recoverWith { case error =>
        Future.sequence((stdoutStream.toList ++ stderrStream.toList).map(_.close())).flatMap(_ => Future.failed(error))
      }
      val result = encoded.flatMap { jsInput =>
        val stdinEndpoints = stdin.map(_ => ToolHostApi.createStdin())
        stdinEndpoints.foreach { case (writer, _, closed) => pumpTransport.pump(stdin.get, writer, closed) }
        val started = rpc.asyncInvokeAndAwait(
          commandPath.toJSArray,
          jsInput,
          stdinEndpoints.map(_._2).orUndefined,
          stdoutEndpoints.map(_._1).orUndefined,
          stderrEndpoints.map(_._1).orUndefined
        )
        lease.observe(started)(
          value => Right(WireToolInvokeResult(value.result.toOption.map(SchemaWireInterop.typedFromJs))),
          {
            case js.JavaScriptException(error) => Left(ToolHostApi.decodeWireRpcFailure(error))
            case scala.util.control.NonFatal(error)
                if !error.isInstanceOf[java.util.concurrent.CancellationException] =>
              Left(
                WireToolRpcFailure.ProtocolError(s"failed to decode tool result: ${String.valueOf(error.getMessage)}")
              )
            case error => throw error
          },
          {
            case Right(_) | Left(WireToolRpcFailure.RemoteToolError(_)) => true
            case _                                                      => false
          },
          {
            case Right(result) =>
              result.result.foreach(value => SchemaWire.releaseOwned(SchemaWire.typedSchemaValueFromWit(value).value))
            case Left(WireToolRpcFailure.RemoteToolError(error)) =>
              SchemaWire.releaseOwned(SchemaWire.typedSchemaValueFromWit(error.payload).value)
            case _ => ()
          }
        )
      }.recover {
        case scala.util.control.NonFatal(error)
            if !error.isInstanceOf[java.util.concurrent.CancellationException] && !error
              .isInstanceOf[ToolResultDetached] =>
          Left(WireToolRpcFailure.ProtocolError(s"failed to encode tool input: ${String.valueOf(error.getMessage)}"))
      }
      result.onComplete(lease.complete)
      Right(
        WireToolRpcStarted(
          stdoutStream,
          stderrStream,
          lease.result,
          () => lease.cancel(),
          () => lease.detach()
        )
      )
    } catch {
      case js.JavaScriptException(error) => Left(ToolHostApi.decodeWireRpcFailure(error))
      case error: Throwable              => Left(WireToolRpcFailure.ProtocolError(String.valueOf(error.getMessage)))
    }
  }
}

final class ToolRpcConstructionException(val failure: ToolRpcFailure) extends RuntimeException(failure.toString)

/**
 * The Scala.js implementation of [[ToolRpcTransport]] over the
 * `golem:tool/host@0.1.0` `tool-rpc` resource: model values are converted to
 * their wire JS shape, the call is driven through `async-invoke-and-await`'s
 * native async result (yielding the event loop while waiting), and failures are
 * decoded into the platform-neutral [[ToolRpcFailure]] model.
 */
private[golem] final class JsToolRpcTransport(rpc: ToolHostApi.RawToolRpc) extends ToolRpcTransport {

  private implicit val ec: scala.concurrent.ExecutionContext =
    ToolInvokerRuntime.executionContext

  def start(
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream],
    stdout: Boolean,
    stderr: Boolean
  ): Either[ToolRpcFailure, ToolRpcStarted] = {
    val prepared = encodeInput(input)

    prepared match {
      case Left(failure) =>
        Left(failure)
      case Right(jsInput) =>
        try {
          val stdinEndpoints  = stdin.map(_ => ToolHostApi.createStdin())
          val stdoutEndpoints = if (stdout) Some(ToolHostApi.createOutput()) else None
          val stderrEndpoints = if (stderr) Some(ToolHostApi.createOutput()) else None
          stdinEndpoints.foreach { case (writer, _, closed) => pump(stdin.get, writer, closed) }
          val observer = rpc.asyncInvokeAndAwait(
            commandPath.toJSArray,
            jsInput,
            stdinEndpoints.map(_._2).orUndefined,
            stdoutEndpoints.map(_._1).orUndefined,
            stderrEndpoints.map(_._1).orUndefined
          )
          val lease = new ToolResultObserver[Either[ToolRpcFailure, ToolInvokeResult]]
          Right(
            ToolRpcStarted(
              stdoutEndpoints.map(e => new JsToolInputStream(e._2)),
              stderrEndpoints.map(e => new JsToolInputStream(e._2)),
              lease.observe(observer)(
                decodeResult,
                {
                  case js.JavaScriptException(error) => Left(ToolHostApi.decodeRpcFailure(error))
                  case error                         => throw error
                },
                {
                  case Right(_) | Left(ToolRpcFailure.RemoteToolError(_)) => true
                  case _                                                  => false
                },
                {
                  case Right(result)                                                                               => result.result.foreach(value => SchemaWire.releaseOwned(value.value))
                  case Left(ToolRpcFailure.RemoteToolError(golem.tool.ToolInvokeError.UnknownToolError(_, value))) =>
                    SchemaWire.releaseOwned(value.value)
                  case Left(ToolRpcFailure.RemoteToolError(golem.tool.ToolInvokeError.Tool(value))) =>
                    SchemaWire.releaseOwned(value.value)
                  case _ => ()
                }
              ),
              () => lease.cancel(),
              () => lease.detach()
            )
          )
        } catch {
          case js.JavaScriptException(e) =>
            Left(ToolHostApi.decodeRpcFailure(e))
        }
    }
  }

  def trigger(
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream]
  ): Either[ToolRpcFailure, Unit] =
    encodeInput(input).flatMap { encoded =>
      try {
        val endpoints = stdin.map(_ => ToolHostApi.createStdin())
        endpoints.foreach { case (writer, _, closed) => pump(stdin.get, writer, closed) }
        rpc.invoke(commandPath.toJSArray, encoded, endpoints.map(_._2).orUndefined)
        Right(())
      } catch {
        case js.JavaScriptException(error)      => Left(ToolHostApi.decodeRpcFailure(error))
        case scala.util.control.NonFatal(error) =>
          Left(ToolRpcFailure.ProtocolError(String.valueOf(error.getMessage)))
      }
    }

  private def encodeInput(input: TypedSchemaValue): Either[ToolRpcFailure, JsTypedSchemaValue] =
    try Right(SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(input)))
    catch {
      case t: Throwable =>
        Left(ToolRpcFailure.ProtocolError(s"failed to encode tool input: ${String.valueOf(t.getMessage)}"))
    }

  private[golem] def pump(
    source: ToolInputStream,
    writer: ToolHostApi.RawToolStdinWriter,
    closed: ToolHostApi.RawToolStdinClosed
  ): Unit = {
    object End
    val acquisition                                                = source.stream.startAsync.start
    val reader                                                     = acquisition.toFuture
    val hostClosed: Future[Option[Either[ByteStreamFailure, Any]]] =
      FutureInterop.fromPromise(closed.waitClosed()).map(_ => None)

    def cleanup(): Future[Unit] =
      reader.value match {
        case Some(Success(active)) => active.close().toFuture
        case Some(Failure(_))      => Future.successful(())
        case None                  =>
          acquisition.cancel()
          source.cancel().recover { case _ => () }
      }

    def loop(): Future[Unit] =
      Future
        .firstCompletedOf(
          List(
            reader
              .flatMap(_.read[Any](End).toFuture)
              .map(value => Option(Right(value): Either[ByteStreamFailure, Any]))
              .recover { case error: StreamError => Option(Left(error.value.asInstanceOf[ByteStreamFailure])) },
            hostClosed
          )
        )
        .flatMap {
          case None                                                    => source.cancel().recover { case _ => () }
          case Some(Right(value)) if value.asInstanceOf[AnyRef] eq End => FutureInterop.fromPromise(writer.finish())
          case Some(Right(byte: Byte))                                 =>
            FutureInterop
              .fromPromise(writer.write(js.typedarray.Uint8Array.from(js.Array((byte & 0xff).toShort))))
              .flatMap(_ => loop())
          case Some(Left(failure)) => FutureInterop.fromPromise(writer.fail(encodeFailure(failure)))
          case Some(Right(other))  => Future.failed(new IllegalStateException(s"unexpected stdin stream value: $other"))
        }
    loop().transformWith {
      case Success(_)     => cleanup()
      case Failure(error) => cleanup().transformWith(_ => Future.failed(error))
    }.recover { case _ => () }
    ()
  }

  private def encodeFailure(failure: ByteStreamFailure): js.Any = failure match {
    case ByteStreamFailure.Cancelled         => js.Dynamic.literal(tag = "cancelled")
    case ByteStreamFailure.Abandoned         => js.Dynamic.literal(tag = "abandoned")
    case ByteStreamFailure.ResourceExhausted => js.Dynamic.literal(tag = "resource-exhausted")
    case ByteStreamFailure.Failed(message)   => js.Dynamic.literal(tag = "failed", `val` = message)
  }

  private def decodeResult(result: JsInvocationResult): Either[ToolRpcFailure, ToolInvokeResult] =
    try {
      if (result.stdout.isDefined || result.stderr.isDefined)
        Left(ToolRpcFailure.ProtocolError("tool result unexpectedly contained output streams"))
      else
        Right(
          ToolInvokeResult(
            result.result.toOption.map(js => SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(js)))
          )
        )
    } catch {
      case t: Throwable =>
        Left(ToolRpcFailure.ProtocolError(s"failed to decode tool result: ${String.valueOf(t.getMessage)}"))
    }
}

/**
 * Owns one host result observer, independently of invocation and output
 * lifetimes.
 */
private final class ToolResultObserver[A] {
  private implicit val ec: scala.concurrent.ExecutionContext = ToolInvokerRuntime.executionContext
  private var observer                                       = Option.empty[ToolHostApi.RawToolFutureInvokeResult]
  private var detached                                       = false
  private var disposed                                       = false
  private var cancelled                                      = false
  private val delivered                                      = Promise[A]()

  val result: Future[A] = delivered.future

  def complete(outcome: Try[A]): Unit = { delivered.tryComplete(outcome); () }

  def cancel(): Unit =
    if (!disposed) {
      cancelled = true
      observer.foreach(_.cancel())
    }

  def detach(): Unit = {
    detached = true
    delivered.tryFailure(new ToolResultDetached)
    ()
  }

  def observe(raw: ToolHostApi.RawToolFutureInvokeResult)(
    decode: JsInvocationResult => A,
    failure: Throwable => A,
    ownsCapabilities: A => Boolean,
    discard: A => Unit
  ): Future[A] = {
    observer = Some(raw)
    def dispose(): Unit = {
      disposed = true
      observer = None
      val symbol = js.Dynamic.global.Symbol.selectDynamic("dispose")
      val drop   = js.Dynamic.global.Reflect.applyDynamic("get")(raw, symbol)
      drop.applyDynamic("call")(raw)
      ()
    }
    val cancellation = Try(if (cancelled) raw.cancel())
    if (detached) {
      AgentStreamOwnership.cleanup { dispose(); Future.successful(()) }
    } else {
      val getting = cancellation match {
        case Failure(error) => Future.failed(error)
        case Success(_)     =>
          try FutureInterop.fromPromise(raw.get())
          catch { case scala.util.control.NonFatal(error) => Future.failed(error) }
      }
      getting.onComplete { result =>
        val ownership = new AgentStreamOwnership
        val decoded   = Try(AgentStreamOwnership.capture(ownership)(result.fold(failure, decode))).recover {
          case scala.util.control.NonFatal(error) => AgentStreamOwnership.capture(ownership)(failure(error))
        }
        if (!detached && delivered.tryComplete(decoded) && decoded.toOption.exists(ownsCapabilities))
          ownership.handoff()
        else {
          AgentStreamOwnership.cleanup { decoded.foreach(discard); Future.successful(()) }
          AgentStreamOwnership.cleanup(ownership.close())
        }
        AgentStreamOwnership.cleanup { dispose(); Future.successful(()) }
      }
    }
    delivered.future
  }
}
