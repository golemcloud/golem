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

import zio.blocks.schema.Schema

import java.util.UUID

/**
 * The ID of an agent: an opaque UUID that Golem issues. It names an agent
 * within one environment.
 *
 * The text form is the hyphenated 8-4-4-4-12 UUID. [[AgentId.parse]] accepts
 * hexadecimal digits in either case and normalizes them to lower case.
 */
final case class AgentId(uuid: UUID) {
  override def toString: String = uuid.toString
}

object AgentId {
  def parse(text: String): Either[String, AgentId] =
    CanonicalUuid
      .parse(text.map(c => if (c >= 'A' && c <= 'Z') (c + ('a' - 'A')).toChar else c))
      .left
      .map(_ => s"Invalid agent ID, expected a hyphenated UUID: $text")
      .map(AgentId(_))

  implicit val schema: Schema[AgentId] = Schema.derived
}
