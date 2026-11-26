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

import golem.host.PermissionsApi.ScopeCard
import golem.schema.GuestPermissionCardHandle
import golem.host.js.schema.JsSchemaValueTree
import golem.runtime.rpc.host.WasmRpcApi
import zio.ZIO
import zio.test._

import scala.scalajs.js

object ScopeCardRpcSpec extends ZIOSpecDefault {

  private val invocationIdRecord = js.Dynamic.literal(
    owner = js.Dynamic.literal(
      tag = "agent",
      `val` = js.Dynamic.literal(uuid = js.Dynamic.literal(highBits = js.BigInt("5"), lowBits = js.BigInt("6")))
    ),
    id = js.Dynamic.literal(highBits = js.BigInt("1"), lowBits = js.BigInt("2"))
  )

  private final class Recorder {
    val scopeCards = scala.collection.mutable.ListBuffer.empty[js.Any]
    val triggered  = scala.collection.mutable.ListBuffer.empty[String]
    val rebuilt    = scala.collection.mutable.ListBuffer.empty[js.Dynamic]

    def install(): Unit = {
      val metadata = js.Dynamic.literal(
        agentId = js.Dynamic.literal(uuid = js.Dynamic.literal(highBits = js.BigInt("5"), lowBits = js.BigInt("6"))),
        invocationId = invocationIdRecord
      )
      val handle = js.Dynamic.literal(
        id = (() => invocationIdRecord): js.Function0[js.Any],
        status = (() => js.Promise.resolve[String]("pending")): js.Function0[js.Promise[String]],
        await_ = (() => js.Promise.resolve[js.Any](js.undefined)): js.Function0[js.Promise[js.Any]],
        cancel = (() => js.Promise.resolve[Boolean](false)): js.Function0[js.Promise[Boolean]]
      )
      js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(
        js.Dynamic.literal(
          wasmRpcInvokeAndAwait = (
            (_: String, _: js.Any, scopeCard: js.Any) => {
              scopeCards += scopeCard
              js.Promise.resolve[js.Any](js.Dynamic.literal(metadata = metadata, result = js.undefined))
            }
          ): js.Function3[String, js.Any, js.Any, js.Any],
          wasmRpcTrigger = (
            (method: String, _: js.Any) => {
              triggered += s"trigger $method"
              js.Promise.resolve[js.Any](handle)
            }
          ): js.Function2[String, js.Any, js.Any],
          wasmRpcSchedule = (
            (_: js.Any, method: String, _: js.Any) => {
              triggered += s"schedule $method"
              js.Promise.resolve[js.Any](handle)
            }
          ): js.Function3[js.Any, String, js.Any, js.Any],
          invocationGet = (
            (id: js.Dynamic) => {
              rebuilt += id
              handle
            }
          ): js.Function1[js.Dynamic, js.Any]
        )
      )
    }
  }

  private def invoker(): RemoteAgentClient.WasmRpcInvoker =
    new RemoteAgentClient.WasmRpcInvoker(
      WasmRpcApi
        .upsert("Target", js.Dynamic.literal().asInstanceOf[JsSchemaValueTree])
        .fold(error => throw new IllegalStateException(error.toString), identity),
      None
    )

  private val input = js.Dynamic.literal().asInstanceOf[JsSchemaValueTree]

  private def card(raw: js.Object): ScopeCard =
    new ScopeCard(GuestPermissionCardHandle.fromRaw(raw))

  override def spec = suite("ScopeCardRpcSpec")(
    test("awaited calls pass the borrowed raw card to the host and leave the card usable") {
      val recorder = new Recorder
      recorder.install()
      val raw   = js.Dynamic.literal(name = "scope")
      val scope = card(raw)
      val rpc   = invoker()
      ZIO.fromFuture { implicit ec =>
        for {
          first <- rpc.invokeAndAwait("m", input, Some(scope))
          _     <- rpc.invokeAndAwait("m", input, Some(scope))
          _     <- rpc.invokeAndAwait("m", input, None)
        } yield assertTrue(
          first.value.isEmpty,
          recorder.scopeCards.size == 3,
          recorder.scopeCards.take(2).forall(_ eq raw),
          js.isUndefined(recorder.scopeCards(2)),
          scope.isPresent
        )
      }
    },
    test("a moved scope card fails loudly before the host is called") {
      val recorder = new Recorder
      recorder.install()
      val scope = card(js.Dynamic.literal(name = "moved"))
      scope.release()
      val rpc = invoker()
      ZIO.fromFuture(_ => rpc.invokeAndAwait("m", input, Some(scope))).flip.map { error =>
        assertTrue(error.isInstanceOf[IllegalStateException], recorder.scopeCards.isEmpty)
      }
    },
    test("trigger and schedule pass no scope card and return the host's handle") {
      val recorder = new Recorder
      recorder.install()
      val rpc = invoker()
      ZIO.fromFuture { implicit ec =>
        for {
          triggered <- rpc.trigger("m", input)
          scheduled <- rpc.schedule(golem.Datetime.fromEpochMillis(1.0), "m", input)
          status    <- triggered.status()
        } yield assertTrue(
          recorder.triggered.toList == List("trigger m", "schedule m"),
          recorder.scopeCards.isEmpty,
          triggered.id == scheduled.id,
          triggered.id.uuid == new java.util.UUID(1L, 2L),
          status == InvocationStatus.Pending
        )
      }
    },
    test("reflection clients pass the borrowed scope card and report a moved one as Left") {
      val recorder = new Recorder
      recorder.install()
      val raw    = js.Dynamic.literal(name = "reflected-scope")
      val scope  = card(raw)
      val moved  = card(js.Dynamic.literal(name = "reflected-moved"))
      val client = golem.reflection.DynamicAgentClient
        .upsert("Target", golem.schema.SchemaValue.StringValue("t"))
        .fold(error => throw new IllegalStateException(error.toString), identity)
      moved.release()
      val method = client.method("m")
      ZIO.fromFuture { implicit ec =>
        for {
          attached <- method.invokeValue(golem.schema.SchemaValue.StringValue("x"), Some(scope))
          rejected <- method.invokeValue(golem.schema.SchemaValue.StringValue("x"), Some(moved))
        } yield assertTrue(
          attached.isRight,
          recorder.scopeCards.size == 1,
          recorder.scopeCards.head eq raw,
          scope.isPresent,
          rejected.left.exists(_.isInstanceOf[golem.reflection.GolemReflectError.Validation])
        )
      }
    },
    test("Invocation.get rebuilds a Unit handle from its ID without a remote call") {
      val recorder = new Recorder
      recorder.install()
      val id         = golem.InvocationId(golem.AgentId(new java.util.UUID(5L, 6L)), new java.util.UUID(1L, 2L))
      val invocation = Invocation.get[Unit](id)
      ZIO.fromFuture { implicit ec =>
        for {
          result    <- invocation.awaitResult()
          cancelled <- invocation.cancel()
        } yield assertTrue(
          invocation.id == id,
          result == (),
          !cancelled,
          recorder.rebuilt.size == 1,
          recorder.rebuilt.head.owner.tag.asInstanceOf[String] == "agent",
          recorder.triggered.isEmpty,
          recorder.scopeCards.isEmpty
        )
      }
    },
    test("reflection clients trigger without a scope card and return a handle") {
      val recorder = new Recorder
      recorder.install()
      val client = golem.reflection.DynamicAgentClient
        .upsert("Target", golem.schema.SchemaValue.StringValue("t"))
        .fold(error => throw new IllegalStateException(error.toString), identity)
      ZIO.fromFuture { implicit ec =>
        client.method("m").triggerValue(golem.schema.SchemaValue.StringValue("x")).flatMap {
          case Left(error)       => scala.concurrent.Future.failed(new IllegalStateException(error.message))
          case Right(invocation) =>
            invocation.awaitResult().map { result =>
              assertTrue(result.isEmpty, recorder.triggered.size == 1, recorder.scopeCards.isEmpty)
            }
        }
      }
    }
  ) @@ TestAspect.sequential
}
