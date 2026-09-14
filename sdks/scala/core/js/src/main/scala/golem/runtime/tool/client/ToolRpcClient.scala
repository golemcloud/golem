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
import golem.schema.TypedSchemaValue
import golem.schema.wire.SchemaWire
import golem.tool._

import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.JSConverters._

/**
 * Entry point generated typed tool clients use to obtain the RPC transport of
 * one remote tool.
 */
object ToolRpcClient {

  /** A transport bound to one remote tool name. */
  def transport(toolName: String): ToolRpcTransport =
    new JsToolRpcTransport(new ToolHostApi.RawToolRpc(toolName))
}

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
    stdout: Boolean
  ): Either[ToolRpcFailure, ToolRpcStarted] = {
    val prepared = encodeInput(input)

    prepared match {
      case Left(failure) =>
        Left(failure)
      case Right(jsInput) =>
        try {
          val stdinEndpoints  = stdin.map(_ => ToolHostApi.createStdin())
          val stdoutEndpoints = if (stdout) Some(ToolHostApi.createStdout()) else None
          stdinEndpoints.foreach { case (writer, _, closed) => pump(stdin.get, writer, closed) }
          val observer = rpc.asyncInvokeAndAwait(
            commandPath.toJSArray,
            jsInput,
            stdinEndpoints.map(_._2).orUndefined,
            stdoutEndpoints.map(_._1).orUndefined
          )
          Right(
            ToolRpcStarted(
              stdoutEndpoints.map(e => new JsToolInputStream(e._2)),
              awaitFutureResult(observer),
              () => observer.cancel()
            )
          )
        } catch {
          case js.JavaScriptException(e) =>
            Left(ToolHostApi.decodeRpcFailure(e))
        }
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
    val closedF              = FutureInterop.fromPromise(closed.waitClosed()).map(_ => false)
    def loop(): Future[Unit] =
      Future.firstCompletedOf(List(source.read().map(Some(_)), closedF.map(_ => None))).flatMap {
        case None                                      => source.cancel().recover { case _ => () }
        case Some(Right(None))                         => FutureInterop.fromPromise(writer.finish())
        case Some(Right(Some(bytes))) if bytes.isEmpty => loop()
        case Some(Right(Some(bytes)))                  =>
          FutureInterop
            .fromPromise(writer.write(js.typedarray.Uint8Array.from(bytes.map(_.toShort).toJSArray)))
            .flatMap(_ => loop())
        case Some(Left(failure)) => FutureInterop.fromPromise(writer.fail(encodeFailure(failure)))
      }
    loop().recover { case _ => () }
    ()
  }

  private def encodeFailure(failure: ByteStreamFailure): js.Any = failure match {
    case ByteStreamFailure.Cancelled         => js.Dynamic.literal(tag = "cancelled")
    case ByteStreamFailure.Abandoned         => js.Dynamic.literal(tag = "abandoned")
    case ByteStreamFailure.ResourceExhausted => js.Dynamic.literal(tag = "resource-exhausted")
    case ByteStreamFailure.Failed(message)   => js.Dynamic.literal(tag = "failed", `val` = message)
  }

  /** Drives the host `future-invoke-result` to completion. */
  private def awaitFutureResult(
    futureResult: ToolHostApi.RawToolFutureInvokeResult
  ): Future[Either[ToolRpcFailure, ToolInvokeResult]] =
    try {
      FutureInterop
        .fromPromise(futureResult.get())
        .map(result => decodeResult(result))
        .recover { case js.JavaScriptException(e) => Left(ToolHostApi.decodeRpcFailure(e)) }
    } catch {
      case js.JavaScriptException(e) =>
        Future.successful(Left(ToolHostApi.decodeRpcFailure(e)))
    }

  private def decodeResult(result: JsInvocationResult): Either[ToolRpcFailure, ToolInvokeResult] =
    try
      Right(
        ToolInvokeResult(
          result.result.toOption.map(js => SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(js)))
        )
      )
    catch {
      case t: Throwable =>
        Left(ToolRpcFailure.ProtocolError(s"failed to decode tool result: ${String.valueOf(t.getMessage)}"))
    }
}
