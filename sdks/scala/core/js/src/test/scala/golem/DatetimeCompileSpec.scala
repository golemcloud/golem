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

import zio.test._

object DatetimeCompileSpec extends ZIOSpecDefault {

  def spec = suite("DatetimeCompileSpec")(
    test("Datetime.fromEpochMillis constructs value") {
      val dt: Datetime = Datetime.fromEpochMillis(1700000000000.0)
      assertTrue(dt.epochMillis == 1700000000000.0)
    },
    test("Datetime.fromEpochSeconds converts correctly") {
      val dt = Datetime.fromEpochSeconds(1700000000.0)
      assertTrue(dt.epochMillis == 1700000000000.0)
    },
    test("Datetime.now returns non-zero value") {
      val dt = Datetime.now
      assertTrue(dt.epochMillis > 0.0)
    },
    test("Datetime.afterMillis is in the future") {
      val before = Datetime.now.epochMillis
      val dt     = Datetime.afterMillis(10000.0)
      assertTrue(dt.epochMillis >= before)
    },
    test("Datetime.afterSeconds is in the future") {
      val before = Datetime.now.epochMillis
      val dt     = Datetime.afterSeconds(10.0)
      assertTrue(dt.epochMillis >= before)
    },
    test("DatetimeJs.fromTs creates Datetime") {
      val dt: Datetime = DatetimeJs.fromTs(1700000000000.0)
      assertTrue(dt.epochMillis == 1700000000000.0)
    },
    test("Datetime is a value type (AnyVal)") {
      val d: Datetime = Datetime.fromEpochMillis(0.0)
      assertTrue(d.epochMillis == 0.0)
    },
    test("Uuid construction and field access") {
      val u = Uuid(BigInt(123456789L), BigInt(987654321L))
      assertTrue(
        u.highBits == BigInt(123456789L),
        u.lowBits == BigInt(987654321L)
      )
    },
    test("Uuid equality") {
      val a = Uuid(BigInt(1), BigInt(2))
      val b = Uuid(BigInt(1), BigInt(2))
      val c = Uuid(BigInt(1), BigInt(3))
      assertTrue(
        a == b,
        a != c
      )
    }
  )
}
