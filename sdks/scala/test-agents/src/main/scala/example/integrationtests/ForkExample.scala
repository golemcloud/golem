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
@description("Demonstrates fork() with the promise-based join pattern from the Golem docs.")
trait ForkDemo extends BaseAgent {

  class Id(val value: String)

  @description("Forks the agent, joins via a promise, and returns info about both branches.")
  def runFork(): Future[String]

  @description("Forks the agent and joins using JSON-encoded promise (awaitPromiseJson/completePromiseJson).")
  def runForkJson(): Future[String]
}
