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

package golem.runtime.rpc.host

import golem.Datetime
import golem.host.js.{JsDatetime, JsResult}
import golem.host.js.schema.{JsAgentError, JsSchemaValueTree, JsTypedAgentConfigValue, JsUuid}
import golem.runtime.rpc.{CancellationToken, RawCancellationToken}

import scala.annotation.unused
import scala.scalajs.js
import scala.scalajs.js.annotation.{JSImport, JSName}

// ---------------------------------------------------------------------------
// `golem:agent/host@2.0.0` `rpc-error` JS facade (schema-native variant).
//
// The `remote-agent-error` payload carries the schema `JsAgentError` (whose
// `custom-error` is a `JsTypedSchemaValue`); the string-carrying cases follow
// the wasm-rquickjs `{ tag, val }` shape.
// ---------------------------------------------------------------------------

@js.native
sealed trait JsRpcError extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsRpcErrorString extends JsRpcError {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsRpcErrorRemoteAgent extends JsRpcError {
  @JSName("val") def value: JsAgentError = js.native
}

private[golem] object WasmRpcApi {
  def newClient(
    agentTypeName: String,
    constructorPayload: JsSchemaValueTree,
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

  private[rpc] def decodeRpcError(thrown: Any): RpcError = {
    // Read the discriminator defensively: a thrown value that is not the
    // expected `{ tag, val }` JS object (e.g. a bare string or a foreign error)
    // must degrade to `unknown` rather than triggering a hard cast failure.
    val rawTag =
      try thrown.asInstanceOf[js.Dynamic].selectDynamic("tag")
      catch { case _: Throwable => (js.undefined: js.Any) }

    if (js.typeOf(rawTag) == "string") {
      val value = thrown.asInstanceOf[JsRpcError]
      rawTag.asInstanceOf[String] match {
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
    } else {
      RpcError("unknown", Some(String.valueOf(thrown)))
    }
  }

  final class WasmRpcClient private[host] (private val underlying: js.Object) {
    def invokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree
    ): Either[RpcError, js.UndefOr[JsSchemaValueTree]] =
      try Right(raw.invokeAndAwait(functionName, input))
      catch {
        case js.JavaScriptException(e) =>
          Left(decodeRpcError(e))
      }

    def invoke(functionName: String, input: JsSchemaValueTree): Either[RpcError, Unit] =
      try {
        raw.invoke(functionName, input)
        Right(())
      } catch {
        case js.JavaScriptException(e) =>
          Left(decodeRpcError(e))
      }

    def asyncInvokeAndAwait(functionName: String, input: JsSchemaValueTree): Either[RpcError, RawFutureInvokeResult] =
      try Right(raw.asyncInvokeAndAwait(functionName, input))
      catch {
        case js.JavaScriptException(e) =>
          Left(decodeRpcError(e))
      }

    def scheduleInvocation(
      datetime: Datetime,
      functionName: String,
      input: JsSchemaValueTree
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
      input: JsSchemaValueTree
    ): Either[RpcError, CancellationToken] =
      try
        Right(
          CancellationToken(
            raw
              .scheduleCancelableInvocation(datetimeToJs(datetime), functionName, input)
              .asInstanceOf[RawCancellationToken]
          )
        )
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
        case Some(err) =>
          val rendered =
            try js.JSON.stringify(err.asInstanceOf[js.Any])
            catch { case _: Throwable => String.valueOf(err) }
          s"$kind: $rendered"
        case None => message.fold(kind)(text => s"$kind: $text")
      }
  }

  @js.native
  @JSImport("golem:agent/host@2.0.0", "FutureInvokeResult")
  private[rpc] class RawFutureInvokeResult extends js.Object {
    def subscribe(): AgentHostApi.Pollable                                     = js.native
    def get(): js.UndefOr[JsResult[js.UndefOr[JsSchemaValueTree], JsRpcError]] = js.native
    def cancel(): Unit                                                         = js.native
  }

  @js.native
  @JSImport("golem:agent/host@2.0.0", "WasmRpc")
  private final class RawWasmRpc(
    @unused agentTypeName: String,
    @unused constructor_ : JsSchemaValueTree,
    @unused phantomId: js.Any,
    @unused agentConfig: js.Array[JsTypedAgentConfigValue]
  ) extends js.Object {
    def invokeAndAwait(methodName: String, input: JsSchemaValueTree): js.UndefOr[JsSchemaValueTree]                   = js.native
    def invoke(methodName: String, input: JsSchemaValueTree): Unit                                                    = js.native
    def asyncInvokeAndAwait(methodName: String, input: JsSchemaValueTree): RawFutureInvokeResult                      = js.native
    def scheduleInvocation(scheduledTime: JsDatetime, methodName: String, input: JsSchemaValueTree): Unit             = js.native
    def scheduleCancelableInvocation(scheduledTime: JsDatetime, methodName: String, input: JsSchemaValueTree): js.Any =
      js.native
  }
}
