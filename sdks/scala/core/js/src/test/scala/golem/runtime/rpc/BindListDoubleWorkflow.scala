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

package golem.runtime.rpc

import golem.runtime.annotations.{DurabilityMode, agentDefinition}
import golem.BaseAgent

/**
 * Top-level agent trait used to regression-test Scala.js AgentClient.bind
 * behavior for collection parameter types.
 */
@agentDefinition("BindListDouble", mode = DurabilityMode.Durable)
trait BindListDoubleWorkflow extends BaseAgent {
  class Id()
  def finished(results: List[Double]): Unit
}
