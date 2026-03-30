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

import golem.runtime.annotations.{DurabilityMode, agentDefinition, agentImplementation, description}
import golem.BaseAgent

import scala.concurrent.Future

/**
 * Minimal, Scala-only example matching the intended SDK user experience.
 *
 * Any packaging/deploy plumbing remains repo-local and is not part of the user
 * story.
 */
@agentDefinition(mode = DurabilityMode.Durable)
trait Shard extends BaseAgent {

  class Id(val tableName: String, val shardId: Int)

  @description("Get a value from the table")
  def get(key: String): Future[Option[String]]

  @description("Set a value in the table")
  def set(key: String, value: String): Unit
}

@agentImplementation()
final class ShardImpl(private val tableName: String, private val shardId: Int) extends Shard {
  override def get(key: String): Future[Option[String]] =
    Future.successful(Some(s"$tableName:$shardId:$key"))

  override def set(key: String, value: String): Unit = ()
}
