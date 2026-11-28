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
import golem.InvocationId
import golem.host.js.{JsDatetime, JsUuid}
import golem.host.js.schema.JsSchemaValueTree
import golem.runtime.rpc.{AgentRpcError, AgentRpcException, Invocation, InvocationStatus}
import zio.ZIO
import zio.test._

import scala.scalajs.js

object WasmRpcApiSpec extends ZIOSpecDefault {

  private val invocationUuid = java.util.UUID.fromString("3f2a1b0c-9d8e-4f7a-b6c5-d4e3f2a1b0c9")
  private val agentId        = golem.AgentId(java.util.UUID.fromString("0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d"))
  private val invocationId   = InvocationId(agentId, invocationUuid)

  private def invocationIdRecord: js.Object =
    js.Dynamic.literal(
      "owner" -> js.Dynamic.literal("tag" -> "agent", "val" -> golem.host.js.JsAgentId.fromAgentId(agentId)),
      "id"    -> JsUuid.fromUuid(invocationUuid)
    )

  private def rawInvocation(
    status: () => js.Promise[String] = () => js.Promise.resolve[String]("scheduled"),
    awaitResult: () => js.Promise[js.UndefOr[JsSchemaValueTree]] = () =>
      js.Promise.resolve[js.UndefOr[JsSchemaValueTree]](js.undefined),
    cancel: () => js.Promise[Boolean] = () => js.Promise.resolve[Boolean](true)
  ): js.Object =
    js.Dynamic.literal(
      "id"     -> (() => invocationIdRecord),
      "status" -> status,
      "await_" -> awaitResult,
      "cancel" -> cancel
    )

  private def rejected[A](tag: String, detail: String): js.Promise[A] =
    js.Promise.reject(js.Dynamic.literal("tag" -> tag, "val" -> detail)).asInstanceOf[js.Promise[A]]

  private def scheduleAt(epochMillis: Double): ZIO[Any, Throwable, (JsDatetime, InvocationId)] = {
    var captured: JsDatetime = null
    val schedule             =
      (scheduledTime: JsDatetime, _: String, _: JsSchemaValueTree) => {
        captured = scheduledTime
        js.Promise.resolve[js.Object](rawInvocation())
      }
    val client = new WasmRpcApi.WasmRpcClient(js.Dynamic.literal("schedule" -> schedule).asInstanceOf[js.Object])
    ZIO
      .fromFuture(_ =>
        client.schedule(Datetime.fromEpochMillis(epochMillis), "agent.method", null.asInstanceOf[JsSchemaValueTree])
      )
      .map(result => (captured, Invocation.fromHost(result.toOption.get).id))
  }

  private def handle(raw: js.Object): Invocation[Option[JsSchemaValueTree]] =
    Invocation.fromHost(raw.asInstanceOf[WasmRpcApi.RawInvocation])

  private def failure(future: scala.concurrent.Future[?]): ZIO[Any, Nothing, Option[AgentRpcError]] =
    ZIO
      .fromFuture(_ => future)
      .flip
      .map {
        case AgentRpcException(error) => Some(error)
        case _                        => None
      }
      .orElseSucceed(None)

  def spec = suite("WasmRpcApiSpec")(
    test("schedule normalizes a pre-epoch instant for the P3 system clock") {
      scheduleAt(-1.0).map { case (captured, id) =>
        assertTrue(id == invocationId, captured.seconds == js.BigInt(-1), captured.nanoseconds == 999000000)
      }
    },
    test("schedule preserves large finite epoch milliseconds as a normalized P3 instant") {
      scheduleAt(9223372036854774784.0).map { case (captured, id) =>
        assertTrue(
          id == invocationId,
          captured.seconds == js.BigInt("9223372036854774"),
          captured.nanoseconds == 784000000
        )
      }
    },
    test("schedule does not double-count a rounded large-epoch second") {
      scheduleAt(9000000000000022528.0).map { case (captured, _) =>
        assertTrue(captured.seconds == js.BigInt("9000000000000022"), captured.nanoseconds == 528000000)
      }
    },
    test("schedule keeps sub-nanosecond negative epochs normalized") {
      scheduleAt(-1e-20).map { case (captured, _) =>
        assertTrue(captured.nanoseconds >= 0, captured.nanoseconds < 1000000000)
      }
    },
    test("a handle reports its status, its result and its cancellation") {
      val invocation = handle(rawInvocation())
      for {
        status    <- ZIO.fromFuture(_ => invocation.status())
        result    <- ZIO.fromFuture(_ => invocation.awaitResult())
        cancelled <- ZIO.fromFuture(_ => invocation.cancel())
      } yield assertTrue(
        invocation.id == invocationId,
        status == InvocationStatus.Scheduled,
        result.isEmpty,
        cancelled
      )
    },
    test("handle operations fail with typed rpc-errors") {
      val invocation = handle(
        rawInvocation(
          status = () => rejected("not-found", "gone"),
          awaitResult = () => rejected("cancelled", "withdrawn"),
          cancel = () => rejected("cannot-cancel-running", "running")
        )
      )
      for {
        status <- failure(invocation.status())
        result <- failure(invocation.awaitResult())
        cancel <- failure(invocation.cancel())
      } yield assertTrue(
        status.contains(AgentRpcError.NotFound("gone")),
        result.contains(AgentRpcError.Cancelled("withdrawn")),
        cancel.contains(AgentRpcError.CannotCancelRunning("running"))
      )
    }
  )
}
