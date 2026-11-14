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

object InvocationIdSpec extends ZIOSpecDefault {
  private val uuid = UUID.fromString("12345678-9abc-4def-8123-456789abcdef")

  def spec = suite("InvocationIdSpec")(
    test("round-trips an agent ID that contains '.', '/' and quotes") {
      val id   = InvocationId("""Shop("a.b/c", "it's \"x\"").v2""", uuid)
      val text = id.toString
      assertTrue(
        text == """Shop("a.b/c", "it's \"x\"").v2.12345678-9abc-4def-8123-456789abcdef""",
        InvocationId.parse(text) == Right(id)
      )
    },
    test("rejects text without a '.' before the UUID") {
      assertTrue(
        InvocationId.parse(s"agent$uuid").isLeft,
        InvocationId.parse(s"agent-$uuid").isLeft,
        InvocationId.parse(uuid.toString).isLeft
      )
    },
    test("rejects an invalid UUID") {
      assertTrue(
        InvocationId.parse("agent.not-a-uuid").isLeft,
        InvocationId.parse("agent.12345678-9abc-4def-8123-456789abcdeg").isLeft,
        InvocationId.parse("agent.12345678-9ABC-4DEF-8123-456789ABCDEG").isLeft,
        InvocationId.parse("agent.123456789abc4def8123456789abcdef0000").isLeft,
        InvocationId.parse("agent.{2345678-9abc-4def-8123-456789abcde}").isLeft
      )
    },
    test("accepts upper-case UUID hex and normalizes it to lower case") {
      val parsed = InvocationId.parse("Shop(\"a.b\").12345678-9ABC-4DEF-8123-456789ABCDEF")
      assertTrue(
        parsed == Right(InvocationId("Shop(\"a.b\")", uuid)),
        parsed.map(_.toString) == Right("Shop(\"a.b\").12345678-9abc-4def-8123-456789abcdef")
      )
    },
    test("rejects an empty agent ID") {
      assertTrue(
        InvocationId.parse(s".$uuid").isLeft,
        InvocationId.parse("").isLeft
      )
    }
  )
}
