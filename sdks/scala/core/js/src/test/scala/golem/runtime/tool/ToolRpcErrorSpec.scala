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

package golem.runtime.tool

import golem.host.ToolWireInterop
import golem.runtime.tool.host.ToolHostApi
import golem.schema.{IntoSchema, TypedSchemaValue}
import golem.schema.wire.SchemaWire
import golem.tool.{ToolInvokeError, ToolRpcFailure}
import golem.tool.wire.WitToolError
import zio.test._

import scala.scalajs.js

/**
 * Verifies the `golem:tool/host@0.1.0` `rpc-error` decoding used by the typed
 * tool client transport: the string-carrying cases, the `remote-tool-error`
 * payload round trip, and the defensive fallback for foreign thrown values.
 */
object ToolRpcErrorSpec extends ZIOSpecDefault {

  private def variant(tag: String, value: js.Any): js.Any =
    js.Dynamic.literal("tag" -> tag, "val" -> value).asInstanceOf[js.Any]

  private def payload(text: String): TypedSchemaValue =
    implicitly[IntoSchema[String]].toTyped(text)

  def spec = suite("ToolRpcErrorSpec")(
    test("decodes the string-carrying error cases") {
      assertTrue(
        ToolHostApi.decodeRpcFailure(variant("protocol-error", "bad frame")) ==
          ToolRpcFailure.ProtocolError("bad frame"),
        ToolHostApi.decodeRpcFailure(variant("denied", "nope")) ==
          ToolRpcFailure.Denied("nope"),
        ToolHostApi.decodeRpcFailure(variant("not-found", "missing")) ==
          ToolRpcFailure.NotFound("missing"),
        ToolHostApi.decodeRpcFailure(variant("remote-internal-error", "boom")) ==
          ToolRpcFailure.RemoteInternalError("boom")
      )
    },
    test("decodes remote-tool-error preserving the custom-error payload") {
      val original = payload("bad flag")
      val jsError  = ToolWireInterop.toolErrorToJs(
        WitToolError.CustomError(SchemaWire.typedSchemaValueToWit(original))
      )
      val decoded = ToolHostApi.decodeRpcFailure(variant("remote-tool-error", jsError))
      decoded match {
        case ToolRpcFailure.RemoteToolError(ToolInvokeError.Custom(roundTripped)) =>
          assertTrue(roundTripped == original)
        case other =>
          assertNever(s"expected remote tool custom error, got: $other")
      }
    },
    test("decodes remote-tool-error framing cases") {
      val jsError = ToolWireInterop.toolErrorToJs(WitToolError.InvalidInput("bad wire input"))
      assertTrue(
        ToolHostApi.decodeRpcFailure(variant("remote-tool-error", jsError)) ==
          ToolRpcFailure.RemoteToolError(ToolInvokeError.InvalidInput("bad wire input"))
      )
    },
    test("falls back to a protocol error for an unrecognized thrown value") {
      assertTrue(
        ToolHostApi.decodeRpcFailure("not a js error") ==
          ToolRpcFailure.ProtocolError("not a js error"),
        ToolHostApi.decodeRpcFailure(variant("mystery", "x")) ==
          ToolRpcFailure.ProtocolError("unknown rpc error `mystery`")
      )
    }
  )
}
