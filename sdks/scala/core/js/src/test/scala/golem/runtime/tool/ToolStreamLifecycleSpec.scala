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

import golem.host.js.tool.{JsByteStreamIterator, JsWasiInputStream, JsWasiOutputStream}
import zio.ZIO
import zio.test.*

import scala.scalajs.js

object ToolStreamLifecycleSpec extends ZIOSpecDefault {
  private final case class StreamFixture(stream: js.Object, closeCount: () => Int, iteratorCount: () => Int)

  private def stream(withReturn: Boolean): StreamFixture = {
    var closes    = 0
    var iterators = 0
    val done      = js.Dynamic.literal("done" -> true, "value" -> 0)
    val iterator  = js.Dynamic.literal(
      "next" -> (((() => js.Promise.resolve(done)): js.Function0[js.Promise[js.Dynamic]]))
    )
    if (withReturn)
      iterator.updateDynamic("return")(
        ((() => {
          closes += 1
          js.Promise.resolve(done)
        }): js.Function0[js.Promise[js.Dynamic]])
      )
    val value = js.Dynamic.literal()
    js.Dynamic.global.Reflect.set(
      value,
      js.Symbol.asyncIterator,
      ((() => {
        iterators += 1
        iterator.asInstanceOf[JsByteStreamIterator]
      }): js.Function0[JsByteStreamIterator])
    )
    StreamFixture(value.asInstanceOf[js.Object], () => closes, () => iterators)
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolStreamLifecycleSpec")(
      test("input and output wrappers close their JS iterators at most once") {
        val inputFixture  = stream(withReturn = true)
        val outputFixture = stream(withReturn = true)
        val input         = new JsToolInputStream(inputFixture.stream.asInstanceOf[JsWasiInputStream])
        val output        = new JsToolOutputStream(outputFixture.stream.asInstanceOf[JsWasiOutputStream])
        for {
          _ <- ZIO.fromFuture(_ => input.close())
          _ <- ZIO.fromFuture(_ => input.close())
          _ <- ZIO.fromFuture(_ => output.close())
          _ <- ZIO.fromFuture(_ => output.close())
        } yield assertTrue(
          inputFixture.closeCount() == 1,
          inputFixture.iteratorCount() == 1,
          outputFixture.closeCount() == 1,
          outputFixture.iteratorCount() == 1
        )
      },
      test("closing a JS stream without iterator return is a successful no-op") {
        val fixture = stream(withReturn = false)
        val input   = new JsToolInputStream(fixture.stream.asInstanceOf[JsWasiInputStream])
        ZIO.fromFuture(_ => input.close()).map(_ => assertTrue(fixture.closeCount() == 0, fixture.iteratorCount() == 1))
      }
    )
}
