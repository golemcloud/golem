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

package demo

import golem.runtime.annotations.{DurabilityMode, agentDefinition, agentImplementation, description, prompt}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition(mount = "/counters/{value}")
trait CounterAgent extends BaseAgent {

  class Id(val value: String)

  @prompt("Increase the count by one")
  @description("Increases the count by one and returns the new value")
  def increment(): Future[Int]
}

@agentDefinition()
trait Example1 extends BaseAgent {
  class Id(val name: String, val count: Int)

  def run(): Future[String]
}


@agentImplementation()
final case class Example1Impl(name: String, count: Int) extends Example1 {
  import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

  override def run(): Future[String] = {
    val client = CounterAgentClient.get(s"x-${name}")
    client.increment().map { n =>
      s"Result: ${n}"
    }
  }
}
