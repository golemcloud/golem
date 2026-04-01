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

package example.templates

import golem.runtime.annotations.{agentDefinition, description}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition()
@description("Demonstrates agent-to-agent RPC by calling Counter remotely.")
trait RpcClient extends BaseAgent {
  class Id(val value: String)
  @description("Invoke Counter.increment remotely and return the result.")
  def callCounter(counterId: String): Future[Int]
}
