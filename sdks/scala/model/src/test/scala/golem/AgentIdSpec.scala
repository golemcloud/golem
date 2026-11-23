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

object AgentIdSpec extends ZIOSpecDefault {
  private val uuid = UUID.fromString("0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d")

  def spec = suite("AgentIdSpec")(
    test("the text form is the UUID") {
      assertTrue(
        AgentId(uuid).toString == "0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d",
        AgentId.parse("0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d") == Right(AgentId(uuid))
      )
    },
    test("parse normalizes upper-case hex digits") {
      assertTrue(AgentId.parse("0A1B2C3D-4E5F-8A6B-9C7D-0E1F2A3B4C5D") == Right(AgentId(uuid)))
    },
    test("parse rejects readable agent names and unhyphenated UUIDs") {
      assertTrue(
        AgentId.parse("Counter(\"a\")").isLeft,
        AgentId.parse("0a1b2c3d4e5f8a6b9c7d0e1f2a3b4c5d").isLeft,
        AgentId.parse("").isLeft
      )
    }
  )
}
