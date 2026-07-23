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
@description("Demonstrates Guards API, HostApi config get/set, and oplog management.")
trait GuardsDemo extends BaseAgent {

  class Id(val value: String)

  @description("Exercises block-scoped guards: withPersistenceLevel, withRetryPolicy, withIdempotenceMode, atomically.")
  def guardsBlockDemo(): Future[String]

  @description(
    "Exercises resource-style guards: usePersistenceLevel, useRetryPolicy, useIdempotenceMode, markAtomicOperation."
  )
  def guardsResourceDemo(): Future[String]

  @description("Exercises oplog management: getOplogIndex, markBeginOperation, markEndOperation, oplogCommit.")
  def oplogDemo(): Future[String]
}
