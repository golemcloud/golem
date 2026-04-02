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

object UuidSpec extends ZIOSpecDefault {

  def spec = suite("UuidSpec")(
    suite("toStandardString / fromStandardString roundtrip")(
      test("roundtrip with known UUID") {
        val uuid   = Uuid(BigInt("123456789012345678"), BigInt("987654321098765432"))
        val str    = Uuid.toStandardString(uuid)
        val parsed = Uuid.fromStandardString(str)
        assertTrue(parsed == Right(uuid))
      },
      test("roundtrip with zero UUID") {
        val uuid   = Uuid(BigInt(0), BigInt(0))
        val str    = Uuid.toStandardString(uuid)
        val parsed = Uuid.fromStandardString(str)
        assertTrue(
          str == "00000000-0000-0000-0000-000000000000",
          parsed == Right(uuid)
        )
      },
      test("roundtrip with max values") {
        val uuid   = Uuid(BigInt("18446744073709551615"), BigInt("18446744073709551615"))
        val str    = Uuid.toStandardString(uuid)
        val parsed = Uuid.fromStandardString(str)
        assertTrue(parsed == Right(uuid))
      },
      test("standard UUID string format") {
        val uuid = Uuid(
          highBits = BigInt("6605191886512583379"),
          lowBits = BigInt("11855323847904993280")
        )
        val str = Uuid.toStandardString(uuid)
        assertTrue(str.matches("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"))
      }
    ),
    suite("fromStandardString error cases")(
      test("invalid format returns Left") {
        val result = Uuid.fromStandardString("not-a-uuid")
        assertTrue(result.isLeft)
      },
      test("empty string returns Left") {
        val result = Uuid.fromStandardString("")
        assertTrue(result.isLeft)
      }
    )
  )
}
