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
 * Identity of a resolved agent: the component it belongs to and its agent id
 * string. Mirrors the server's `AgentId`.
 */
final case class AgentId(componentId: String, agentId: String)

object AgentId {
  def fromJson(json: Json): Either[String, AgentId] =
    for {
      componentId <- Json.requireField(json, "componentId").flatMap(Json.asString)
      agentId     <- Json.requireField(json, "agentId").flatMap(Json.asString)
    } yield AgentId(componentId, agentId)
}
