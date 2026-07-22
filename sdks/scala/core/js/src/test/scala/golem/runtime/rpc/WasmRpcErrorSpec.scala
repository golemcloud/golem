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

package golem.runtime.rpc

import golem.runtime.rpc.host.WasmRpcApi
import zio.test._

import scala.scalajs.js

/**
 * Verifies the `golem:agent/host@2.0.0` `rpc-error` decoding used by both the
 * synchronous and asynchronous RPC paths. The async path (`readAsyncResult`)
 * reuses [[WasmRpcApi.decodeRpcError]] so that a completed `Err` result keeps
 * the same v2 error surface as a thrown one (instead of `[object Object]`).
 */
object WasmRpcErrorSpec extends ZIOSpecDefault {

  private def variant(tag: String, value: js.Any): js.Any =
    js.Dynamic.literal("tag" -> tag, "val" -> value).asInstanceOf[js.Any]

  def spec = suite("WasmRpcErrorSpec")(
    test("decodes the string-carrying error cases with their messages") {
      val protocol = WasmRpcApi.decodeRpcError(variant("protocol-error", "bad frame"))
      val denied   = WasmRpcApi.decodeRpcError(variant("denied", "nope"))
      val notFound = WasmRpcApi.decodeRpcError(variant("not-found", "missing"))
      val internal = WasmRpcApi.decodeRpcError(variant("remote-internal-error", "boom"))
      assertTrue(
        protocol.kind == "protocol-error",
        protocol.message.contains("bad frame"),
        protocol.toString == "protocol-error: bad frame",
        denied.kind == "denied",
        denied.message.contains("nope"),
        notFound.kind == "not-found",
        notFound.message.contains("missing"),
        internal.kind == "remote-internal-error",
        internal.message.contains("boom")
      )
    },
    test("decodes remote-agent-error preserving the v2 agent-error payload in toString") {
      val agentError = js.Dynamic.literal("tag" -> "invalid-input", "val" -> "field x").asInstanceOf[js.Any]
      val decoded    = WasmRpcApi.decodeRpcError(variant("remote-agent-error", agentError))
      val rendered   = decoded.toString
      assertTrue(
        decoded.kind == "remote-agent-error",
        decoded.message.isEmpty,
        decoded.agentError.isDefined,
        rendered.startsWith("remote-agent-error:"),
        rendered.contains("invalid-input"),
        rendered.contains("field x")
      )
    },
    test("falls back to 'unknown' for an unrecognized thrown value") {
      val decoded = WasmRpcApi.decodeRpcError("not a js error")
      assertTrue(decoded.kind == "unknown")
    }
  )
}
