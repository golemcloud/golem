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
import golem.host.js.JsComponentId
import golem.host.js.schema.JsTypedSchemaValue
import golem.host.js.tool.{JsInvocationResult, JsTool, JsToolError}
import golem.runtime.tool.ToolImplementationRuntime
import golem.tool.{ByteStreamCloseCause, ByteStreamFailure, StreamWriteError, ToolRpcFailure}
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
    def createStdin(): js.Array[js.Any]                     = js.native
    def createStdout(): js.Array[js.Any]                    = js.native
  }

  def createStdin(): (RawToolStdinWriter, RawToolStdin, RawToolStdinClosed) = {
    val endpoints = ToolHostModule.createStdin()
    (
      endpoints(0).asInstanceOf[RawToolStdinWriter],
      endpoints(1).asInstanceOf[RawToolStdin],
      endpoints(2).asInstanceOf[RawToolStdinClosed]
    )
  }

  def createStdout(): (RawToolStdout, RawByteStream) = {
    val endpoints = ToolHostModule.createStdout()
    (endpoints(0).asInstanceOf[RawToolStdout], endpoints(1).asInstanceOf[RawByteStream])
  }

  @js.native
  sealed trait RawByteIteratorResult extends js.Object {
    def done: Boolean = js.native
    def value: js.Any = js.native
  }
  @js.native
  sealed trait RawByteIterator extends js.Object {
    def next(): js.Promise[RawByteIteratorResult]                             = js.native
    @JSName("return") def returnIterator(): js.Promise[RawByteIteratorResult] = js.native
  }
  @js.native
  sealed trait RawByteStream extends js.Object {
    @JSName(js.Symbol.asyncIterator) def asyncIterator(): RawByteIterator = js.native
  }

  @js.native
  sealed trait RawToolStdin extends js.Object
  @js.native
  sealed trait RawToolStdout extends js.Object
  @js.native
  sealed trait RawToolStdinWriter extends js.Object {
    def write(bytes: js.typedarray.Uint8Array): js.Promise[Unit] = js.native
    def finish(): js.Promise[Unit]                               = js.native
    def fail(reason: js.Any): js.Promise[Unit]                   = js.native
  }
  @js.native
  sealed trait RawToolStdinClosed extends js.Object {
    @JSName("wait") def waitClosed(): js.Promise[js.Any] = js.native
  }
  @js.native
  sealed trait RawToolStdoutWriter extends js.Object {
    def write(bytes: js.typedarray.Uint8Array): js.Promise[Unit] = js.native
    def finish(): js.Promise[Unit]                               = js.native
    def fail(reason: js.Any): js.Promise[Unit]                   = js.native
  }

  @js.native
  @JSImport("golem:tool/host@0.1.0", "ToolRpc")
  final class RawToolRpc(@unused toolName: String) extends js.Object {
    def invokeAndAwait(
      commandPath: js.Array[String],
      input: JsTypedSchemaValue,
      stdin: js.UndefOr[RawToolStdin],
      stdout: js.UndefOr[RawToolStdout]
    ): js.Promise[JsInvocationResult] = js.native

    def invoke(
      commandPath: js.Array[String],
      input: JsTypedSchemaValue,
      stdin: js.UndefOr[RawToolStdin]
    ): Unit = js.native

    def asyncInvokeAndAwait(
      commandPath: js.Array[String],
      input: JsTypedSchemaValue,
      stdin: js.UndefOr[RawToolStdin],
      stdout: js.UndefOr[RawToolStdout]
    ): RawToolFutureInvokeResult = js.native
  }

  @js.native
  @JSImport("golem:tool/host@0.1.0", "FutureInvokeResult")
  final class RawToolFutureInvokeResult extends js.Object {
    def get(): js.Promise[JsInvocationResult] = js.native
    def cancel(): Unit                        = js.native
  }

  /** Decodes a rejected `stream-write-error` Promise value when recognized. */
  def decodeStreamWriteError(thrown: Any): Option[StreamWriteError] =
    variantTag(thrown).flatMap {
      case "concurrent-operation" => Some(StreamWriteError.ConcurrentOperation)
      case "closed"               =>
        variantValue(thrown).flatMap(decodeByteStreamCloseCause).map(StreamWriteError.Closed(_))
      case _ => None
    }

  private def decodeByteStreamCloseCause(value: Any): Option[ByteStreamCloseCause] =
    variantTag(value).flatMap {
      case "finished"           => Some(ByteStreamCloseCause.Finished)
      case "consumer-cancelled" => Some(ByteStreamCloseCause.ConsumerCancelled)
      case "failed"             =>
        variantValue(value).flatMap(decodeByteStreamFailure).map(ByteStreamCloseCause.Failed(_))
      case _ => None
    }

  private def decodeByteStreamFailure(value: Any): Option[ByteStreamFailure] =
    variantTag(value).flatMap {
      case "cancelled"          => Some(ByteStreamFailure.Cancelled)
      case "abandoned"          => Some(ByteStreamFailure.Abandoned)
      case "resource-exhausted" => Some(ByteStreamFailure.ResourceExhausted)
      case "failed"             => variantValue(value).collect { case message: String => ByteStreamFailure.Failed(message) }
      case _                    => None
    }

  private def variantTag(value: Any): Option[String] =
    try {
      val tag = value.asInstanceOf[js.Dynamic].selectDynamic("tag")
      if (js.typeOf(tag) == "string") Some(tag.asInstanceOf[String]) else None
    } catch { case _: Throwable => None }

  private def variantValue(value: Any): Option[Any] =
    try {
      val nested = value.asInstanceOf[js.Dynamic].selectDynamic("val")
      if (js.isUndefined(nested)) None else Some(nested)
    } catch { case _: Throwable => None }

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
        case "cancelled"          => ToolRpcFailure.Cancelled
        case "resource-exhausted" =>
          ToolRpcFailure.ResourceExhausted(thrown.asInstanceOf[JsToolRpcErrorString].value)
        case other =>
          ToolRpcFailure.ProtocolError(s"unknown rpc error `$other`")
      }
    } else {
      ToolRpcFailure.ProtocolError(String.valueOf(thrown))
    }
  }
}
