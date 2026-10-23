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

package golem

import zio.test._

import java.util.UUID

object CanonicalUuidSpec extends ZIOSpecDefault {

  def spec = suite("CanonicalUuidSpec")(
    test("parses nil and max UUIDs") {
      assertTrue(
        CanonicalUuid.parse("00000000-0000-0000-0000-000000000000") == Right(new UUID(0L, 0L)),
        CanonicalUuid.parse("ffffffff-ffff-ffff-ffff-ffffffffffff") == Right(new UUID(-1L, -1L))
      )
    },
    test("round-trips the text that UUID.toString produces") {
      val uuid = UUID.fromString("12345678-9abc-4def-8123-456789abcdef")
      assertTrue(CanonicalUuid.parse(uuid.toString) == Right(uuid))
    },
    test("rejects non-canonical text") {
      assertTrue(
        CanonicalUuid.parse("FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF").isLeft,
        CanonicalUuid.parse("1-2-3-4-5").isLeft,
        CanonicalUuid.parse("123456789abc4def8123456789abcdef").isLeft,
        CanonicalUuid.parse("{12345678-9abc-4def-8123-456789abcdef}").isLeft,
        CanonicalUuid.parse("").isLeft
      )
    }
  )
}
