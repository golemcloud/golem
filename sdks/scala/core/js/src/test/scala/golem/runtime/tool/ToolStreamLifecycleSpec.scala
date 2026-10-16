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
import golem.runtime.tool.client.JsToolRpcTransport
import golem.runtime.tool.host.ToolHostApi
import golem.tool.{ByteStreamFailure, ToolInputStream}
import zio.blocks.async.*
import zio.blocks.streams.{JvmType, Stream}
import zio.ZIO
import zio.test.*

import scala.concurrent.{Future, Promise}
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
        val input         = new JsMiddlewareInputStream(inputFixture.stream.asInstanceOf[JsWasiInputStream])
        val output        = new JsMiddlewareOutputStream(outputFixture.stream.asInstanceOf[JsWasiOutputStream])
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
        val input   = new JsMiddlewareInputStream(fixture.stream.asInstanceOf[JsWasiInputStream])
        ZIO.fromFuture(_ => input.close()).map(_ => assertTrue(fixture.closeCount() == 0, fixture.iteratorCount() == 1))
      },
      test("tool input cancellation lazily closes its JS iterator at most once") {
        val fixture = stream(withReturn = true)
        val input   = new JsToolInputStream(fixture.stream.asInstanceOf[ToolHostApi.RawByteStream])
        for {
          _ <- ZIO.succeed(assertTrue(fixture.iteratorCount() == 0))
          _ <- ZIO.fromFuture(_ => input.cancel())
          _ <- ZIO.fromFuture(_ => input.cancel())
          _ <- ZIO.fromFuture(_ => input.close())
        } yield assertTrue(fixture.closeCount() == 1, fixture.iteratorCount() == 1)
      },
      test("host stdin closure cancels a custom input source exactly once") {
        val blocked = Promise[Unit]()
        var cancels = 0
        val input   = new ToolInputStream {
          override val stream: Stream[ByteStreamFailure, Byte] = Stream.unfoldAsync(()) { _ =>
            val waiting: Async[Unit] = Async.fromFuture(blocked.future)
            waiting.map(_ => Option.empty[(Byte, Unit)])
          }(using JvmType.Infer.byte)
          override def cancel(): Future[Unit] = {
            cancels += 1
            blocked.trySuccess(())
            Future.successful(())
          }
        }
        val writer = js.Dynamic
          .literal(
            "write"  -> ((_: js.typedarray.Uint8Array) => js.Promise.resolve(())).asInstanceOf[js.Function1[?, ?]],
            "finish" -> (() => js.Promise.resolve(())).asInstanceOf[js.Function0[?]],
            "fail"   -> ((_: js.Any) => js.Promise.resolve(())).asInstanceOf[js.Function1[?, ?]]
          )
          .asInstanceOf[ToolHostApi.RawToolStdinWriter]
        val closed = js.Dynamic
          .literal("wait" -> js.Any.fromFunction0(() => js.Promise.resolve(())))
          .asInstanceOf[ToolHostApi.RawToolStdinClosed]

        new JsToolRpcTransport(null).pump(input, writer, closed)
        ZIO.yieldNow.repeatN(10).as(assertTrue(cancels == 1))
      }
    )
}
