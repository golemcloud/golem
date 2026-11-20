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
  private val uuid    = UUID.fromString("12345678-9abc-4def-8123-456789abcdef")
  private val agentId = AgentId(UUID.fromString("0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d"))

  def spec = suite("InvocationIdSpec")(
    test("round-trips the <agent-id>.<uuid> text form") {
      val id   = InvocationId(agentId, uuid)
      val text = id.toString
      assertTrue(
        text == "0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d.12345678-9abc-4def-8123-456789abcdef",
        InvocationId.parse(text) == Right(id)
      )
    },
    test("rejects text without a '.' between the two UUIDs") {
      assertTrue(
        InvocationId.parse(s"$agentId$uuid").isLeft,
        InvocationId.parse(s"$agentId-$uuid").isLeft,
        InvocationId.parse(uuid.toString).isLeft
      )
    },
    test("rejects an agent part that is not a UUID") {
      assertTrue(
        InvocationId.parse("Shop(a.b)." + uuid).isLeft,
        InvocationId.parse(s"0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5g.$uuid").isLeft
      )
    },
    test("rejects an invalid invocation UUID") {
      assertTrue(
        InvocationId.parse(s"$agentId.not-a-uuid").isLeft,
        InvocationId.parse(s"$agentId.12345678-9abc-4def-8123-456789abcdeg").isLeft,
        InvocationId.parse(s"$agentId.12345678-9ABC-4DEF-8123-456789ABCDEG").isLeft,
        InvocationId.parse(s"$agentId.123456789abc4def8123456789abcdef0000").isLeft,
        InvocationId.parse(s"$agentId.{2345678-9abc-4def-8123-456789abcde}").isLeft
      )
    },
    test("accepts upper-case UUID hex and normalizes it to lower case") {
      val parsed = InvocationId.parse("0A1B2C3D-4E5F-8A6B-9C7D-0E1F2A3B4C5D.12345678-9ABC-4DEF-8123-456789ABCDEF")
      assertTrue(
        parsed == Right(InvocationId(agentId, uuid)),
        parsed.map(_.toString) == Right("0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d.12345678-9abc-4def-8123-456789abcdef")
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
