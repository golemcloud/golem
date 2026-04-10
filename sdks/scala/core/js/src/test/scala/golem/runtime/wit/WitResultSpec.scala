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

package golem.runtime.wit

import zio.test._

object WitResultSpec extends ZIOSpecDefault {
  def spec = suite("WitResultSpec")(
    test("ok unwraps to value and stays Ok through map") {
      val result = WitResult.ok(21)
      val mapped = result.map(_ * 2)

      assertTrue(
        mapped.isOk,
        mapped.unwrap() == 42,
        mapped.toEither == Right(42)
      )
    },
    test("err unwrapErr returns payload and mapError transforms it") {
      val err    = WitResult.err("boom")
      val mapped = err.mapError(_.toUpperCase)

      assertTrue(
        mapped.isErr,
        mapped.unwrapErr() == "BOOM",
        mapped.toEither == Left("BOOM")
      )
    },
    test("flatMap short-circuits on error") {
      val first  = WitResult.ok(1)
      val second = WitResult.err[String]("fail")

      val combined = for {
        a <- first
        _ <- second
      } yield a + 1

      assertTrue(combined.isErr) &&
      assertTrue {
        try {
          combined.unwrap()
          false
        } catch {
          case _: UnwrapError => true
        }
      }
    },
    test("unwrapForWit rethrows Throwable payloads directly") {
      val boom = new IllegalStateException("boom")
      val err  = WitResult.err[Throwable](boom)

      assertTrue {
        try {
          err.unwrapForWit()
          false
        } catch {
          case ex: IllegalStateException => ex eq boom
        }
      }
    },
    test("unwrapForWit wraps non-throwable payloads") {
      val err = WitResult.err("boom")

      assertTrue {
        try {
          err.unwrapForWit()
          false
        } catch {
          case ex: UnwrapError => ex.payload == "boom"
        }
      }
    }
  )
}
