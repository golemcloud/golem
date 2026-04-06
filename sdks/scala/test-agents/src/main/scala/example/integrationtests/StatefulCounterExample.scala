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

final case class CounterState(initialCount: Int)

object CounterState {
  implicit val schema: zio.blocks.schema.Schema[CounterState] = zio.blocks.schema.Schema.derived
}

@agentDefinition()
@description("Counter whose constructor takes a custom case class (CounterState) instead of String.")
trait StatefulCounter extends BaseAgent {

  class Id(val initialCount: Int)

  @description("Increments the counter and returns the new value.")
  def increment(): Future[Int]

  @description("Returns the current count without modifying it.")
  def current(): Future[Int]
}

@agentDefinition()
@description("Calls StatefulCounter remotely to exercise agent-to-agent RPC with a custom state type.")
trait StatefulCaller extends BaseAgent {

  class Id(val initialCount: Int)

  @description("Increments the remote stateful counter and returns its new value.")
  def remoteIncrement(): Future[Int]
}
