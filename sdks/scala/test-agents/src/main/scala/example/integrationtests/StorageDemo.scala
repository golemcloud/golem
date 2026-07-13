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

package example.integrationtests

import golem.runtime.annotations.{agentDefinition, description}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition()
@description("Demonstrates KeyValue bucket CRUD, Blobstore container/object operations, and Config reading.")
trait StorageDemo extends BaseAgent {

  class Id(val value: String)

  @description("Full KeyValue lifecycle: open bucket, set/get/exists/keys/delete.")
  def keyValueDemo(): Future[String]

  @description("Full Blobstore lifecycle: create container, write/read/list/delete objects.")
  def blobstoreDemo(): Future[String]

  @description("Read config values with typed error handling.")
  def configDemo(): Future[String]
}
