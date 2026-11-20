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

package golem.bridge.runtime

import golem.bridge.runtime.json.Json

/**
 * The ID of an agent: an opaque UUID that Golem issues. Its text form is the
 * hyphenated UUID, which is also its JSON form.
 */
final case class AgentId(uuid: String) {
  override def toString: String = uuid
}

object AgentId {
  private val UuidPattern = "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"

  /** Parses the text form, a hyphenated UUID. */
  def parse(value: String): Either[String, AgentId] =
    if (value.matches(UuidPattern)) Right(AgentId(value.toLowerCase))
    else Left(s"Invalid agent ID: $value is not a UUID")

  def fromJson(json: Json): Either[String, AgentId] =
    Json.asString(json).flatMap(parse)
}
