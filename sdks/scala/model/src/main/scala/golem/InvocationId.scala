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

import java.util.UUID

/**
 * Identifies one invocation of an agent: the agent ID and the UUID of the
 * invocation within that agent.
 *
 * The text form is `<agent-id>.<uuid>`, where both parts are hyphenated
 * 8-4-4-4-12 UUIDs. [[InvocationId.parse]] accepts hexadecimal digits in either
 * case and normalizes them to lower case.
 */
final case class InvocationId(agentId: AgentId, uuid: UUID) {
  override def toString: String = s"$agentId.$uuid"
}

object InvocationId {
  private val UuidLength = 36

  def parse(text: String): Either[String, InvocationId] =
    if (text.length != 2 * UuidLength + 1 || text.charAt(UuidLength) != '.')
      Left(s"Invalid invocation ID, expected <agent-id>.<uuid>: $text")
    else
      for {
        agentId <- AgentId
                     .parse(text.substring(0, UuidLength))
                     .left
                     .map(_ => s"Invalid invocation ID, expected a hyphenated agent ID before the '.': $text")
        uuid <- CanonicalUuid
                  .parse(asciiLowerCase(text.substring(UuidLength + 1)))
                  .left
                  .map(_ => s"Invalid invocation ID, expected a hyphenated UUID after the '.': $text")
      } yield InvocationId(agentId, uuid)

  private def asciiLowerCase(text: String): String =
    text.map(c => if (c >= 'A' && c <= 'Z') (c + ('a' - 'A')).toChar else c)
}
