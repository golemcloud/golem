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

package golem.runtime.tool.host

import golem.host.ToolWireInterop
import golem.host.js.{JsComponentId, JsResult}
import golem.host.js.schema.JsTypedSchemaValue
import golem.host.js.tool.{JsInvocationResult, JsTool, JsToolError, JsWasiInputStream}
import golem.runtime.rpc.host.AgentHostApi
import golem.runtime.tool.ToolImplementationRuntime
import golem.tool.ToolRpcFailure
import golem.tool.wire.WitTool

import scala.annotation.unused
import scala.scalajs.js
import scala.scalajs.js.annotation.{JSImport, JSName}

// ---------------------------------------------------------------------------
// `golem:tool/host@0.1.0` `rpc-error` JS facade: the string-carrying cases
// follow the wasm-rquickjs `{ tag, val }` shape; `remote-tool-error` carries
// the wire `tool-error` payload.
// ---------------------------------------------------------------------------

@js.native
sealed trait JsToolRpcError extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsToolRpcErrorString extends JsToolRpcError {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsToolRpcErrorTool extends JsToolRpcError {
  @JSName("val") def value: JsToolError = js.native
}

/** JS shape of the host `registered-tool` record. */
@js.native
sealed trait JsRegisteredTool extends js.Object {
  def definition: JsTool           = js.native
  def implementedBy: JsComponentId = js.native
}

/**
 * Scala.js facade of the `golem:tool/host@0.1.0` interface: ambient tool
 * discovery (`get-all-tools` / `get-tool`), the `tool-rpc` resource and its
 * async invocation future, plus the decoding of thrown `rpc-error` values into
 * the platform-neutral [[ToolRpcFailure]] model.
 */
private[golem] object ToolHostApi {

  /**
   * A tool registered in the environment: its decoded wire descriptor and the
   * component that implements it.
   */
  final case class RegisteredTool(definition: WitTool, implementedBy: JsComponentId)

  /**
   * Every tool the calling agent has access to in the current environment
   * (per-caller access filtering is applied by the host). Order is unspecified.
   */
  def getAllTools(): List[RegisteredTool] =
    ToolHostModule.getAllTools().toList.map(decodeRegisteredTool)

  /**
   * The registered tool with the given name, iff the calling agent has access
   * to it; `None` when the tool is not registered or not accessible (the two
   * cases are not distinguished).
   */
  def getTool(name: String): Option[RegisteredTool] =
    ToolHostModule.getTool(name).toOption.map(decodeRegisteredTool)

  private def decodeRegisteredTool(raw: JsRegisteredTool): RegisteredTool =
    RegisteredTool(ToolWireInterop.toolFromJs(raw.definition), raw.implementedBy)

  @js.native
  @JSImport("golem:tool/host@0.1.0", JSImport.Namespace)
  private object ToolHostModule extends js.Object {
    def getAllTools(): js.Array[JsRegisteredTool]           = js.native
    def getTool(name: String): js.UndefOr[JsRegisteredTool] = js.native
  }

  @js.native
  @JSImport("golem:tool/host@0.1.0", "ToolRpc")
  final class RawToolRpc(@unused toolName: String) extends js.Object {
    def invokeAndAwait(
      commandPath: js.Array[String],
      input: JsTypedSchemaValue,
      stdin: js.UndefOr[JsWasiInputStream]
    ): JsInvocationResult = js.native

    def invoke(
      commandPath: js.Array[String],
      input: JsTypedSchemaValue,
      stdin: js.UndefOr[JsWasiInputStream]
    ): Unit = js.native

    def asyncInvokeAndAwait(
      commandPath: js.Array[String],
      input: JsTypedSchemaValue,
      stdin: js.UndefOr[JsWasiInputStream]
    ): RawToolFutureInvokeResult = js.native
  }

  @js.native
  @JSImport("golem:tool/host@0.1.0", "FutureInvokeResult")
  final class RawToolFutureInvokeResult extends js.Object {
    def subscribe(): AgentHostApi.Pollable                              = js.native
    def get(): js.UndefOr[JsResult[JsInvocationResult, JsToolRpcError]] = js.native
    def cancel(): Unit                                                  = js.native
  }

  /**
   * Decodes a thrown or returned `rpc-error` value. A value that is not the
   * expected `{ tag, val }` JS object (e.g. a bare string or a foreign error)
   * degrades to a protocol error rather than triggering a hard cast failure.
   */
  def decodeRpcFailure(thrown: Any): ToolRpcFailure = {
    val rawTag =
      try thrown.asInstanceOf[js.Dynamic].selectDynamic("tag")
      catch { case _: Throwable => (js.undefined: js.Any) }

    if (js.typeOf(rawTag) == "string") {
      rawTag.asInstanceOf[String] match {
        case "protocol-error" =>
          ToolRpcFailure.ProtocolError(thrown.asInstanceOf[JsToolRpcErrorString].value)
        case "denied" =>
          ToolRpcFailure.Denied(thrown.asInstanceOf[JsToolRpcErrorString].value)
        case "not-found" =>
          ToolRpcFailure.NotFound(thrown.asInstanceOf[JsToolRpcErrorString].value)
        case "remote-internal-error" =>
          ToolRpcFailure.RemoteInternalError(thrown.asInstanceOf[JsToolRpcErrorString].value)
        case "remote-tool-error" =>
          try {
            val wire = ToolWireInterop.toolErrorFromJs(thrown.asInstanceOf[JsToolRpcErrorTool].value)
            ToolRpcFailure.RemoteToolError(ToolImplementationRuntime.errorFromWire(wire))
          } catch {
            case t: Throwable =>
              ToolRpcFailure.ProtocolError(
                s"failed to decode remote tool error: ${String.valueOf(t.getMessage)}"
              )
          }
        case other =>
          ToolRpcFailure.ProtocolError(s"unknown rpc error `$other`")
      }
    } else {
      ToolRpcFailure.ProtocolError(String.valueOf(thrown))
    }
  }
}
