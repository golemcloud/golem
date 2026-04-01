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

import golem.Result._
import zio.test.{assertTrue, ZIOSpecDefault}

object ResultCompileSpec extends ZIOSpecDefault {

  def spec = suite("ResultCompileSpec")(
    test("Result.ok creates Ok variant") {
      val r: Result[Int, Nothing] = Result.ok(42)
      assertTrue(
        r.isOk,
        r.unwrap() == 42
      )
    },
    test("Result.err creates Err variant") {
      val r: Result[Nothing, String] = Result.err("boom")
      assertTrue(
        r.isErr,
        r.unwrapErr() == "boom"
      )
    },
    test("Result.fromEither converts Right") {
      val r: Result[Int, String] = Result.fromEither(Right(42))
      assertTrue(
        r.isOk,
        r.unwrap() == 42
      )
    },
    test("Result.fromEither converts Left") {
      val r: Result[Int, String] = Result.fromEither(Left("fail"))
      assertTrue(
        r.isErr,
        r.unwrapErr() == "fail"
      )
    },
    test("Result.fromOption converts Some") {
      val r = Result.fromOption(Some(42), "missing")
      assertTrue(
        r.isOk,
        r.unwrap() == 42
      )
    },
    test("Result.fromOption converts None") {
      val r = Result.fromOption(None, "missing")
      assertTrue(
        r.isErr,
        r.unwrapErr() == "missing"
      )
    },
    test("Result.toEither roundtrips Ok") {
      val r = Result.ok(42)
      assertTrue(r.toEither == Right(42))
    },
    test("Result.toEither roundtrips Err") {
      val r = Result.err("boom")
      assertTrue(r.toEither == Left("boom"))
    },
    test("Result.map transforms Ok value") {
      val r = Result.ok(21).map(_ * 2)
      assertTrue(r.unwrap() == 42)
    },
    test("Result.map preserves Err") {
      val r: Result[Int, String] = Result.err("fail")
      val mapped                 = r.map(_ * 2)
      assertTrue(mapped.isErr)
    },
    test("Result.flatMap chains Ok values") {
      val r = for {
        a <- Result.ok(10)
        b <- Result.ok(20)
      } yield a + b
      assertTrue(r.unwrap() == 30)
    },
    test("Result.flatMap short-circuits on Err") {
      val r = for {
        a <- Result.ok(10)
        _ <- Result.err[String]("fail")
      } yield a
      assertTrue(r.isErr)
    },
    test("Result.mapError transforms Err") {
      val r = Result.err("lower").mapError(_.toUpperCase)
      assertTrue(r.unwrapErr() == "LOWER")
    },
    test("Result type alias resolves to WitResult") {
      val _: golem.runtime.wit.WitResult[Int, String] = Result.ok[Int](42): Result[Int, String]
      assertTrue(true)
    }
  )
}
