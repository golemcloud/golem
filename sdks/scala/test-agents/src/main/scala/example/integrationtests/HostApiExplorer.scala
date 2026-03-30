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

import golem.runtime.annotations.{agentDefinition, description}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition()
@description("Explores raw host APIs to discover their JS shape for typing.")
trait HostApiExplorer extends BaseAgent {

  class Id(val value: String)

  @description("Explore the WASI config store module")
  def exploreConfig(): Future[String]

  @description("Explore the durability module")
  def exploreDurability(): Future[String]

  @description("Explore the context module")
  def exploreContext(): Future[String]

  @description("Explore the oplog module")
  def exploreOplog(): Future[String]

  @description("Explore the WASI keyvalue module")
  def exploreKeyValue(): Future[String]

  @description("Explore the WASI blobstore module")
  def exploreBlobstore(): Future[String]

  @description("Explore the RDBMS module")
  def exploreRdbms(): Future[String]

  @description("Explore all raw host APIs in one call")
  def exploreAll(): Future[String]
}
