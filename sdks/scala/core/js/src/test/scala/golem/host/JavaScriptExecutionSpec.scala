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

package golem.host

import zio._
import zio.test._

import scala.scalajs.js

object JavaScriptExecutionSpec extends ZIOSpecDefault {
  import JavaScriptExecution._

  private def json(value: js.Any): String = js.JSON.stringify(value)

  private def withHost[A](host: js.Dynamic)(body: => A): A = {
    val previous = js.Dynamic.global.globalThis.selectDynamic("__golemScalaTestHost")
    js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(host)
    try body
    finally js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(previous)
  }

  def spec = suite("JavaScriptExecutionSpec")(
    test("inline source carries its language and only the given options") {
      val options = toJs(
        Options(
          Code.Source("return 1 as number", Language.TypeScript),
          cwd = Some("/work"),
          argv = Some(List("a", "b")),
          env = Some(Map("KEY" -> "value")),
          timeoutMs = Some(500),
          maxBytes = Some(64),
          overflow = Some(Overflow.Truncate)
        )
      )
      assertTrue(
        json(options) ==
          """{"source":"return 1 as number","language":"typescript","cwd":"/work","argv":["a","b"],"env":{"KEY":"value"},"timeoutMs":500,"maxBytes":64,"overflow":"truncate"}"""
      )
    },
    test("an entry has no language and omits unset options") {
      assertTrue(json(toJs(Options(Code.Entry("./main.ts")))) == """{"entry":"./main.ts"}""")
    },
    test("run decodes the collected result") {
      var received: js.Any = js.undefined
      val host             = js.Dynamic.literal(
        runJavaScript = ((options: js.Any) => {
          received = options
          js.Promise.resolve[js.Dynamic](
            js.Dynamic.literal(value = 42, overflowed = false, stdout = "out\n", stderr = "")
          )
        }): js.Function1[js.Any, js.Promise[js.Dynamic]]
      )
      ZIO
        .fromFuture(_ => withHost(host)(JavaScriptExecution.run(Options(Code.Source("return 42")))))
        .map { result =>
          assertTrue(
            json(received) == """{"source":"return 42","language":"javascript"}""",
            result.value.asInstanceOf[Int] == 42,
            !result.overflowed,
            result.stdout == "out\n",
            result.stderr == ""
          )
        }
    },
    test("run fails when the module rejects the options") {
      val host = js.Dynamic.literal(
        runJavaScript = ((_: js.Any) => throw js.JavaScriptException(js.Error("bad options"))): js.Function1[
          js.Any,
          js.Promise[js.Dynamic]
        ]
      )
      ZIO
        .fromFuture(_ => withHost(host)(JavaScriptExecution.run(Options(Code.Entry("x.js")))))
        .exit
        .map(exit => assertTrue(exit.isFailure))
    },
    test("start exposes the result and cancellation") {
      var cancelled = false
      val stream    = js.Dynamic.literal()
      val host      = js.Dynamic.literal(
        startJavaScript = (
          (_: js.Any) =>
            js.Dynamic.literal(
              stdout = stream,
              stderr = stream,
              result = js.Promise.resolve[js.Dynamic](js.Dynamic.literal(value = "done", overflowed = true)),
              cancel = (() => cancelled = true): js.Function0[Unit]
            )
        ): js.Function1[js.Any, js.Dynamic]
      )
      val job = withHost(host)(JavaScriptExecution.start(Options(Code.Entry("job.mjs"))))
      job.cancel()
      ZIO.fromFuture(_ => job.result).map { result =>
        assertTrue(cancelled, result.value.asInstanceOf[String] == "done", result.overflowed)
      }
    }
  )
}
