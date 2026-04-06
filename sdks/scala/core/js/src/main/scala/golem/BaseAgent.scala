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

/**
 * Base trait for Scala agent interfaces.
 *
 * Constructor parameters are defined by declaring an inner `class Id` on the
 * agent trait:
 *
 * {{{
 * @agentDefinition()
 * trait Shard extends BaseAgent {
 *   class Id(val tableName: String, val shardId: Int)
 *   def get(key: String): Future[Option[String]]
 * }
 * }}}
 *
 * When the agent is mounted over HTTP via `@agentDefinition(mount = "...")`,
 * mount path variables must match the `Id` parameter names. Example:
 * `@agentDefinition(mount = "/api/{tableName}/{shardId}")`
 *
 * When running inside Golem, constructor values are provided by the host
 * runtime.
 */
trait BaseAgent {
  final def agentId: String = BaseAgentPlatform.agentId

  final def agentType: String = BaseAgentPlatform.agentType

  final def agentName: String = BaseAgentPlatform.agentName
}
