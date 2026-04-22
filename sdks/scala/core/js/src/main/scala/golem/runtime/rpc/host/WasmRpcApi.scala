/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.runtime.rpc.host

import golem.Datetime
import golem.host.js._
import golem.runtime.rpc.{CancellationToken, RawCancellationToken}

import scala.annotation.unused
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

private[golem] object WasmRpcApi {
  def newClient(
    agentTypeName: String,
    constructorPayload: JsDataValue,
    phantomId: js.UndefOr[JsUuid],
    agentConfig: js.Array[JsTypedAgentConfigValue]
  ): WasmRpcClient = {
    val phantomArg: js.Any = phantomId.getOrElse(js.undefined)
    new WasmRpcClient(new RawWasmRpc(agentTypeName, constructorPayload, phantomArg, agentConfig))
  }

  private def datetimeToJs(datetime: Datetime): JsDatetime = {
    val totalMs = datetime.epochMillis
    val seconds = js.BigInt((totalMs / 1000.0).toLong.toString)
    val nanos   = ((totalMs % 1000.0) * 1e6).toInt
    JsDatetime(seconds, nanos)
  }

  private def decodeRpcError(thrown: Any): RpcError =
    try {
      val value = thrown.asInstanceOf[JsRpcError]
      value.tag match {
        case "protocol-error" =>
          RpcError("protocol-error", Some(value.asInstanceOf[JsRpcErrorString].value))
        case "denied" =>
          RpcError("denied", Some(value.asInstanceOf[JsRpcErrorString].value))
        case "not-found" =>
          RpcError("not-found", Some(value.asInstanceOf[JsRpcErrorString].value))
        case "remote-internal-error" =>
          RpcError("remote-internal-error", Some(value.asInstanceOf[JsRpcErrorString].value))
        case "remote-agent-error" =>
          RpcError("remote-agent-error", None, Some(value.asInstanceOf[JsRpcErrorRemoteAgent].value))
        case other =>
          RpcError(other, None)
      }
    } catch {
      case _: Exception =>
        RpcError("unknown", Some(String.valueOf(thrown)))
    }

  final class WasmRpcClient private[host] (private val underlying: js.Object) {
    def invokeAndAwait(functionName: String, input: JsDataValue): Either[RpcError, JsDataValue] =
      try Right(raw.invokeAndAwait(functionName, input))
      catch {
        case js.JavaScriptException(e) =>
          Left(decodeRpcError(e))
      }

    def invoke(functionName: String, input: JsDataValue): Either[RpcError, Unit] =
      try {
        raw.invoke(functionName, input)
        Right(())
      } catch {
        case js.JavaScriptException(e) =>
          Left(decodeRpcError(e))
      }

    def asyncInvokeAndAwait(functionName: String, input: JsDataValue): Either[RpcError, JsDataValue] =
      try Right(raw.asyncInvokeAndAwait(functionName, input))
      catch {
        case js.JavaScriptException(e) =>
          Left(decodeRpcError(e))
      }

    def scheduleInvocation(
      datetime: Datetime,
      functionName: String,
      input: JsDataValue
    ): Either[RpcError, Unit] =
      try {
        raw.scheduleInvocation(datetimeToJs(datetime), functionName, input)
        Right(())
      } catch {
        case js.JavaScriptException(e) =>
          Left(decodeRpcError(e))
      }

    def scheduleCancelableInvocation(
      datetime: Datetime,
      functionName: String,
      input: JsDataValue
    ): Either[RpcError, CancellationToken] =
      try Right(new CancellationToken(raw.scheduleCancelableInvocation(datetimeToJs(datetime), functionName, input).asInstanceOf[RawCancellationToken]))
      catch {
        case js.JavaScriptException(e) =>
          Left(decodeRpcError(e))
      }

    private def raw: RawWasmRpc =
      underlying.asInstanceOf[RawWasmRpc]
  }

  final case class RpcError(
    kind: String,
    message: Option[String] = None,
    agentError: Option[JsAgentError] = None
  ) {
    override def toString: String =
      agentError match {
        case Some(err) => s"$kind: ${js.JSON.stringify(err.asInstanceOf[js.Any])}"
        case None      => message.fold(kind)(text => s"$kind: $text")
      }
  }

  @js.native
  @JSImport("golem:agent/host@1.5.0", "WasmRpc")
  private final class RawWasmRpc(
    @unused agentTypeName: String,
    @unused constructor_ : JsDataValue,
    @unused phantomId: js.Any,
    @unused agentConfig: js.Array[JsTypedAgentConfigValue]
  ) extends js.Object {
    def invokeAndAwait(methodName: String, input: JsDataValue): JsDataValue                                     = js.native
    def invoke(methodName: String, input: JsDataValue): Unit                                                    = js.native
    def asyncInvokeAndAwait(methodName: String, input: JsDataValue): JsDataValue                                = js.native
    def scheduleInvocation(scheduledTime: JsDatetime, methodName: String, input: JsDataValue): Unit             = js.native
    def scheduleCancelableInvocation(scheduledTime: JsDatetime, methodName: String, input: JsDataValue): js.Any =
      js.native
  }
}
