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

package example.integrationtests

import golem.runtime.annotations.{agentDefinition, description, DurabilityMode}
import golem.BaseAgent
import zio.blocks.schema.Schema

import scala.concurrent.Future

final case class TypedNested(x: Double, tags: List[String])
object TypedNested {
  implicit val schema: Schema[TypedNested] = Schema.derived
}
final case class TypedPayload(
  name: String,
  count: Int,
  note: Option[String],
  flags: List[String],
  nested: TypedNested
)
object TypedPayload {
  implicit val schema: Schema[TypedPayload] = Schema.derived
}
final case class TypedReply(shardName: String, shardIndex: Int, reversed: String, payload: TypedPayload)
object TypedReply {
  implicit val schema: Schema[TypedReply] = Schema.derived
}

@agentDefinition()
@description("A minimal worker agent used for in-Golem agent-to-agent calling examples.")
trait Worker extends BaseAgent {
  class Id(val arg0: String, val arg1: Int)
  def reverse(input: String): Future[String]
  def handle(payload: TypedPayload): Future[TypedReply]
}

@agentDefinition(mode = DurabilityMode.Ephemeral)
@description("A minimal coordinator agent that calls Worker via agent RPC inside Golem.")
trait Coordinator extends BaseAgent {
  class Id(val value: String)
  def route(shardName: String, shardIndex: Int, input: String): Future[String]
  def routeTyped(shardName: String, shardIndex: Int, payload: TypedPayload): Future[TypedReply]
}
