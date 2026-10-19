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

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.scalajs.js.annotation.JSImport
import scala.util.Try

/**
 * Runs JavaScript or TypeScript jobs in fresh QuickJS runtimes through the
 * runtime's `wasm-rquickjs:execution` module.
 *
 * Each job gets fresh globals, process state, timers and module caches, and
 * only the environment variables given in [[Options.env]]. Jobs share the
 * component filesystem and host capabilities, so this is not a security
 * sandbox. TypeScript is transpiled without type checking.
 */
object JavaScriptExecution {

  sealed abstract class Language(val wire: String) extends Product with Serializable
  object Language {
    case object JavaScript extends Language("javascript")
    case object TypeScript extends Language("typescript")
  }

  /** What [[Options.maxBytes]] does when a job writes more output. */
  sealed abstract class Overflow(val wire: String) extends Product with Serializable
  object Overflow {
    case object Terminate extends Overflow("terminate")
    case object Truncate  extends Overflow("truncate")
  }

  sealed trait Code extends Product with Serializable
  object Code {

    /**
     * Inline source that runs as the body of an async function; its return
     * value is the job value.
     */
    final case class Source(source: String, language: Language = Language.JavaScript) extends Code

    /**
     * A module file. Files ending in `.ts`, `.mts` or `.cts` are transformed
     * from TypeScript.
     */
    final case class Entry(path: String) extends Code
  }

  final case class Options(
    code: Code,
    cwd: Option[String] = None,
    argv: Option[List[String]] = None,
    env: Option[Map[String, String]] = None,
    timeoutMs: Option[Int] = None,
    maxBytes: Option[Int] = None,
    overflow: Option[Overflow] = None
  )

  final case class StructuredResult(value: js.Any, overflowed: Boolean)

  final case class Result(value: js.Any, overflowed: Boolean, stdout: String, stderr: String)

  /** A Node.js readable stream of a running job's output. */
  @js.native
  trait OutputStream extends js.Object {
    def on(event: String, listener: js.Function1[js.Any, Unit]): OutputStream = js.native
    def setEncoding(encoding: String): OutputStream                           = js.native
  }

  final class Job private[JavaScriptExecution] (raw: js.Dynamic) {
    val stdout: OutputStream = raw.stdout.asInstanceOf[OutputStream]
    val stderr: OutputStream = raw.stderr.asInstanceOf[OutputStream]

    val result: Future[StructuredResult] =
      raw.result.asInstanceOf[js.Promise[js.Dynamic]].toFuture.map { value =>
        StructuredResult(value.value.asInstanceOf[js.Any], value.overflowed.asInstanceOf[Boolean])
      }

    def cancel(): Unit = raw.cancel()
  }

  @js.native
  @JSImport("wasm-rquickjs:execution", JSImport.Namespace)
  private object ExecutionModule extends js.Object {
    def runJavaScript(options: js.Object): js.Promise[js.Dynamic] = js.native
    def startJavaScript(options: js.Object): js.Dynamic           = js.native
  }

  /** Runs a job to completion and collects its output. */
  def run(options: Options): Future[Result] =
    Future.fromTry(Try(ExecutionModule.runJavaScript(toJs(options)))).flatMap(_.toFuture).map { value =>
      Result(
        value.value.asInstanceOf[js.Any],
        value.overflowed.asInstanceOf[Boolean],
        value.stdout.asInstanceOf[String],
        value.stderr.asInstanceOf[String]
      )
    }

  /**
   * Starts a job and returns its live output streams, result and cancellation.
   */
  def start(options: Options): Job =
    new Job(ExecutionModule.startJavaScript(toJs(options)))

  private[host] def toJs(options: Options): js.Object = {
    val fields = js.Dictionary.empty[js.Any]
    options.code match {
      case Code.Source(source, language) =>
        fields("source") = source
        fields("language") = language.wire
      case Code.Entry(path) =>
        fields("entry") = path
    }
    options.cwd.foreach(fields("cwd") = _)
    options.argv.foreach(argv => fields("argv") = argv.toJSArray)
    options.env.foreach(env => fields("env") = env.toJSDictionary)
    options.timeoutMs.foreach(fields("timeoutMs") = _)
    options.maxBytes.foreach(fields("maxBytes") = _)
    options.overflow.foreach(overflow => fields("overflow") = overflow.wire)
    fields.asInstanceOf[js.Object]
  }
}
