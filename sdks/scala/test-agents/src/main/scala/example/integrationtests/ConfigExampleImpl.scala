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

import golem.config.Config
import golem.runtime.annotations.agentImplementation

import scala.concurrent.Future

@agentImplementation()
final class ConfigAgentImpl(input: String, config: Config[MyAppConfig]) extends ConfigAgent {
  override def greet(): Future[String] = {
    val cfg     = config.value
    val appName = cfg.appName
    val host    = cfg.db.host
    val port    = cfg.db.port
    Future.successful(s"Hello from $appName! DB at $host:$port, input=$input")
  }
}

@agentImplementation()
final class ConfigCallerAgentImpl(input: String) extends ConfigCallerAgent {
  override def callWithOverride(): Future[String] = {
    val configAgent = ConfigAgentClient.getWithConfig(
      input,
      appName = Some("OverriddenApp"),
      dbHost = Some("overridden-host.example.com"),
      dbPort = Some(9999)
    )
    configAgent.greet()
  }
}
