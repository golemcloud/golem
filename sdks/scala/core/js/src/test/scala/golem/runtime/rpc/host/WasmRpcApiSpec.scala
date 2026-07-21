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
import golem.host.js.JsDatetime
import golem.host.js.schema.JsSchemaValueTree
import zio.test._

import scala.scalajs.js

object WasmRpcApiSpec extends ZIOSpecDefault {

  def spec = suite("WasmRpcApiSpec")(
    test("scheduleInvocation normalizes a pre-epoch instant for the P3 system clock") {
      var captured: JsDatetime = null
      val scheduleInvocation =
        (scheduledTime: JsDatetime, _: String, _: JsSchemaValueTree) => captured = scheduledTime
      val raw = js.Dynamic
        .literal("scheduleInvocation" -> scheduleInvocation)
        .asInstanceOf[js.Object]
      val client = new WasmRpcApi.WasmRpcClient(raw)

      val result = client.scheduleInvocation(
        Datetime.fromEpochMillis(-1.0),
        "agent.method",
        null.asInstanceOf[JsSchemaValueTree]
      )

      assertTrue(
        result == Right(()),
        captured.seconds == js.BigInt(-1),
        captured.nanoseconds == 999000000
      )
    },
    test("scheduleInvocation preserves large finite epoch milliseconds as a normalized P3 instant") {
      var captured: JsDatetime = null
      val scheduleInvocation =
        (scheduledTime: JsDatetime, _: String, _: JsSchemaValueTree) => captured = scheduledTime
      val raw = js.Dynamic
        .literal("scheduleInvocation" -> scheduleInvocation)
        .asInstanceOf[js.Object]
      val client = new WasmRpcApi.WasmRpcClient(raw)

      val result = client.scheduleInvocation(
        Datetime.fromEpochMillis(9223372036854774784.0),
        "agent.method",
        null.asInstanceOf[JsSchemaValueTree]
      )

      assertTrue(
        result == Right(()),
        captured.seconds == js.BigInt("9223372036854774"),
        captured.nanoseconds == 784000000
      )
    },
    test("scheduleInvocation does not double-count a rounded large-epoch second") {
      var captured: JsDatetime = null
      val scheduleInvocation =
        (scheduledTime: JsDatetime, _: String, _: JsSchemaValueTree) => captured = scheduledTime
      val raw = js.Dynamic
        .literal("scheduleInvocation" -> scheduleInvocation)
        .asInstanceOf[js.Object]
      val client = new WasmRpcApi.WasmRpcClient(raw)

      val result = client.scheduleInvocation(
        Datetime.fromEpochMillis(9000000000000022528.0),
        "agent.method",
        null.asInstanceOf[JsSchemaValueTree]
      )

      assertTrue(
        result == Right(()),
        captured.seconds == js.BigInt("9000000000000022"),
        captured.nanoseconds == 528000000
      )
    },
    test("scheduleInvocation keeps sub-nanosecond negative epochs normalized") {
      var captured: JsDatetime = null
      val scheduleInvocation =
        (scheduledTime: JsDatetime, _: String, _: JsSchemaValueTree) => captured = scheduledTime
      val raw = js.Dynamic
        .literal("scheduleInvocation" -> scheduleInvocation)
        .asInstanceOf[js.Object]
      val client = new WasmRpcApi.WasmRpcClient(raw)

      val result = client.scheduleInvocation(
        Datetime.fromEpochMillis(-1e-20),
        "agent.method",
        null.asInstanceOf[JsSchemaValueTree]
      )

      assertTrue(
        result == Right(()),
        captured.nanoseconds >= 0,
        captured.nanoseconds < 1000000000
      )
    }
  )
}
