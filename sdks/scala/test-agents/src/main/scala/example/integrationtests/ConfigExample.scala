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

import golem.BaseAgent
import golem.config.AgentConfig
import golem.runtime.annotations.{agentDefinition, description}

import scala.concurrent.Future

@agentDefinition()
@description("Example agent with configuration")
trait ConfigAgent extends BaseAgent with AgentConfig[MyAppConfig] {
  class Id(val value: String)

  @description("Returns a greeting using config values")
  def greet(): Future[String]
}

@agentDefinition()
@description("Example agent that calls ConfigAgent with config overrides")
trait ConfigCallerAgent extends BaseAgent {
  class Id(val value: String)

  @description("Calls ConfigAgent with overridden config values")
  def callWithOverride(): Future[String]
}
