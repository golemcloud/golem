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
import golem.host.js.tool.JsWasiInputStream
import golem.runtime.tool.{JsToolInputStream, JsToolOutputStream}
import golem.runtime.tool.host.ToolHostApi
import golem.schema.TypedSchemaValue
import golem.schema.wire.SchemaWire
import golem.tool.{ToolInputStream, ToolInvokeResult, ToolInvokerRuntime, ToolRpcFailure, ToolRpcTransport}

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

  def invokeAndAwait(
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream]
  ): Future[Either[ToolRpcFailure, ToolInvokeResult]] = {
    val prepared = for {
      jsInput <- encodeInput(input)
      jsStdin <- encodeStdin(stdin)
    } yield (jsInput, jsStdin)

    prepared match {
      case Left(failure) =>
        Future.successful(Left(failure))
      case Right((jsInput, jsStdin)) =>
        try awaitFutureResult(rpc.asyncInvokeAndAwait(commandPath.toJSArray, jsInput, jsStdin))
        catch {
          case js.JavaScriptException(e) =>
            Future.successful(Left(ToolHostApi.decodeRpcFailure(e)))
        }
    }
  }

  private def encodeInput(input: TypedSchemaValue): Either[ToolRpcFailure, JsTypedSchemaValue] =
    try Right(SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(input)))
    catch {
      case t: Throwable =>
        Left(ToolRpcFailure.ProtocolError(s"failed to encode tool input: ${String.valueOf(t.getMessage)}"))
    }

  private def encodeStdin(
    stdin: Option[ToolInputStream]
  ): Either[ToolRpcFailure, js.UndefOr[JsWasiInputStream]] =
    stdin match {
      case None                            => Right(js.undefined)
      case Some(stream: JsToolInputStream) => Right(stream.underlying)
      case Some(other)                     =>
        Left(
          ToolRpcFailure.ProtocolError(
            s"unexpected non-JS tool stdin stream: ${other.getClass.getName}"
          )
        )
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
          result.result.toOption.map(js => SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(js))),
          result.stdout.toOption.map(new JsToolOutputStream(_))
        )
      )
    catch {
      case t: Throwable =>
        Left(ToolRpcFailure.ProtocolError(s"failed to decode tool result: ${String.valueOf(t.getMessage)}"))
    }
}
