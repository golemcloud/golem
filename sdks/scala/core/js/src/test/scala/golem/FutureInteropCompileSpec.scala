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

package golem

import zio._
import zio.test._

import scala.concurrent.Future
import scala.scalajs.js

object FutureInteropCompileSpec extends ZIOSpecDefault {
  def spec = suite("FutureInteropCompileSpec")(
    test("fromPromise converts js.Promise to Future") {
      ZIO.fromFuture { _ =>
        val promise: js.Promise[Int] = js.Promise.resolve[Int](42)
        val future: Future[Int]      = FutureInterop.fromPromise(promise)
        future.map(v => v)(scala.scalajs.concurrent.JSExecutionContext.queue)
      }.map(v => assertTrue(v == 42))
    },
    test("toPromise converts Future to js.Promise") {
      ZIO.fromFuture { implicit ec =>
        val future: Future[String]      = Future.successful("ok")
        val promise: js.Promise[String] = FutureInterop.toPromise(future)
        FutureInterop.fromPromise(promise).map(v => v)
      }.map(v => assertTrue(v == "ok"))
    },
    test("fromEither converts Right to successful Future") {
      ZIO.fromFuture { implicit ec =>
        val future: Future[Int] = FutureInterop.fromEither(Right(42))
        future.map(v => v)
      }.map(v => assertTrue(v == 42))
    },
    test("fromEither converts Left to failed Future") {
      ZIO.fromFuture { implicit ec =>
        val future: Future[Int] = FutureInterop.fromEither(Left("boom"))
        future.failed.map(ex => ex.getMessage)
      }.map(msg => assertTrue(msg.contains("boom")))
    },
    test("failed creates failed Future") {
      ZIO.fromFuture { implicit ec =>
        val future: Future[Int] = FutureInterop.failed("error message")
        future.failed.map(ex => ex.getMessage)
      }.map(msg => assertTrue(msg.contains("error message")))
    },
    test("toPromise then fromPromise roundtrips") {
      ZIO.fromFuture { implicit ec =>
        val original     = Future.successful(List(1, 2, 3))
        val roundtripped = FutureInterop.fromPromise(FutureInterop.toPromise(original))
        roundtripped.map(v => v)
      }.map(v => assertTrue(v == List(1, 2, 3)))
    }
  )
}
