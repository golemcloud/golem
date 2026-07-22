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
@description("Target agent whose methods are called via trigger (fire-and-forget).")
trait TriggerTarget extends BaseAgent {
  class Id(val value: String)

  @description("Multi-param method exercising trigger dispatch.")
  def process(x: Int, label: String): Future[Int]

  @description("No-arg method exercising trigger dispatch.")
  def ping(): Future[String]
}

@agentDefinition()
@description("Calls TriggerTarget methods via trigger (fire-and-forget) and schedule.")
trait TriggerCaller extends BaseAgent {
  class Id(val value: String)

  @description("Fires TriggerTarget.process via trigger and returns confirmation.")
  def fireProcess(): Future[String]

  @description("Fires TriggerTarget.ping via trigger and returns confirmation.")
  def firePing(): Future[String]
}
