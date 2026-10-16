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
import golem.host.js.schema.{JsSchemaValueTree, JsTypedAgentConfigValue}
import golem.runtime.rpc.host.WasmRpcApi
import zio.ZIO
import zio.test._

import scala.scalajs.js
import scala.util.Try

object ScopeCardRpcSpec extends ZIOSpecDefault {

  private final class Recorder {
    val scopeCards = scala.collection.mutable.ListBuffer.empty[js.Any]

    def install(): Unit = {
      val metadata                                             = js.Dynamic.literal(agentId = "Target(\"a\")", idempotencyKey = "k")
      val record: js.Function3[String, js.Any, js.Any, js.Any] = (_, _, scopeCard) => {
        scopeCards += scopeCard
        ()
      }
      js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(
        js.Dynamic.literal(
          wasmRpcInvokeAndAwait = (
            (method: String, input: js.Any, scopeCard: js.Any) => {
              record(method, input, scopeCard)
              js.Dynamic.literal(metadata = metadata, result = js.undefined)
            }
          ): js.Function3[String, js.Any, js.Any, js.Any],
          wasmRpcAsyncInvokeAndAwait = (
            (method: String, input: js.Any, scopeCard: js.Any) => {
              record(method, input, scopeCard)
              js.Dynamic.literal(
                metadata = metadata,
                future = js.Dynamic.literal(
                  get = (() => js.Promise.resolve[js.Any](js.undefined)): js.Function0[js.Promise[js.Any]],
                  cancel = (() => ()): js.Function0[Unit]
                )
              )
            }
          ): js.Function3[String, js.Any, js.Any, js.Any]
        )
      )
    }
  }

  private def invoker(): RemoteAgentClient.WasmRpcInvoker =
    new RemoteAgentClient.WasmRpcInvoker(
      WasmRpcApi.newClient(
        "Target",
        js.Dynamic.literal().asInstanceOf[JsSchemaValueTree],
        js.undefined,
        js.Array[JsTypedAgentConfigValue]()
      )
    )

  private val input = js.Dynamic.literal().asInstanceOf[JsSchemaValueTree]

  private def card(raw: js.Object): ScopeCard =
    new ScopeCard(GuestPermissionCardHandle.fromRaw(raw))

  override def spec = suite("ScopeCardRpcSpec")(
    test("await variants pass the borrowed raw card to the host and leave the card usable") {
      val recorder = new Recorder
      recorder.install()
      val raw   = js.Dynamic.literal(name = "scope")
      val scope = card(raw)
      val rpc   = invoker()
      ZIO.fromFuture { implicit ec =>
        val sync = rpc.invokeAndAwait("m", input, Some(scope))
        val meta = rpc.invokeAndAwaitWithMetadata("m", input, Some(scope))
        for {
          _ <- rpc.asyncInvokeAndAwait("m", input, Some(scope))
          _ <- rpc.cancelableAsyncInvokeAndAwait("m", input, Some(scope))._1
          _ <- FutureInterop.fromEither(rpc.asyncInvokeAndAwaitWithMetadata("m", input, Some(scope))).flatMap(_.result)
          _ <- rpc.asyncInvokeAndAwait("m", input, None)
        } yield assertTrue(
          sync.isRight,
          meta.isRight,
          recorder.scopeCards.size == 6,
          recorder.scopeCards.take(5).forall(_ eq raw),
          js.isUndefined(recorder.scopeCards(5)),
          scope.isPresent
        )
      }
    },
    test("a moved scope card fails loudly before the host is called") {
      val recorder = new Recorder
      recorder.install()
      val scope = card(js.Dynamic.literal(name = "moved"))
      scope.release()
      val rpc   = invoker()
      val sync  = Try(rpc.invokeAndAwait("m", input, Some(scope)))
      val async = Try(rpc.asyncInvokeAndAwait("m", input, Some(scope)))
      assertTrue(
        sync.failed.toOption.exists(_.isInstanceOf[IllegalStateException]),
        async.failed.toOption.exists(_.isInstanceOf[IllegalStateException]),
        recorder.scopeCards.isEmpty
      )
    },
    test("reflection clients pass the borrowed scope card and report a moved one as Left") {
      val recorder = new Recorder
      recorder.install()
      val raw    = js.Dynamic.literal(name = "reflected-scope")
      val scope  = card(raw)
      val moved  = card(js.Dynamic.literal(name = "reflected-moved"))
      val client = golem.reflection.DynamicAgentClient
        .ephemeral("Target", golem.schema.SchemaValue.StringValue("t"))
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
    }
  ) @@ TestAspect.sequential

  private object FutureInterop {
    def fromEither[A](value: Either[String, A]): scala.concurrent.Future[A] =
      value.fold(err => scala.concurrent.Future.failed(new RuntimeException(err)), scala.concurrent.Future.successful)
  }
}
