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
 * Identifies one invocation of an agent: the agent ID in its text form and the
 * UUID of the invocation within that agent.
 *
 * The text form is `<agent-id>.<uuid>`. The agent ID can itself contain `.`, so
 * [[InvocationId.parse]] splits off the fixed-length `.<uuid>` suffix. The UUID
 * must be in the hyphenated 8-4-4-4-12 layout; its hexadecimal digits can be in
 * either case and are normalized to lower case.
 */
final case class InvocationId(agentId: String, uuid: UUID) {
  override def toString: String = s"$agentId.$uuid"
}

object InvocationId {
  private val SuffixLength = 37

  def parse(text: String): Either[String, InvocationId] =
    if (text.length <= SuffixLength)
      Left(s"Invalid invocation ID, expected <agent-id>.<uuid>: $text")
    else {
      val separator = text.length - SuffixLength
      if (text.charAt(separator) != '.')
        Left(s"Invalid invocation ID, missing '.' before the UUID: $text")
      else
        CanonicalUuid
          .parse(asciiLowerCase(text.substring(separator + 1)))
          .left
          .map(_ => s"Invalid invocation ID, expected a hyphenated UUID after the last '.': $text")
          .map(uuid => InvocationId(text.substring(0, separator), uuid))
    }

  private def asciiLowerCase(text: String): String =
    text.map(c => if (c >= 'A' && c <= 'Z') (c + ('a' - 'A')).toChar else c)
}
