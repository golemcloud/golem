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

package golem.runtime

import scala.compiletime.testing.{typeCheckErrors, Error}
import zio.test._

object ReadOnlyEphemeralRejectSpec extends ZIOSpecDefault {

  def spec = suite("ReadOnlyEphemeralRejectSpec")(
    test("ephemeral agent with @readOnly method fails to compile (Scala 3)") {
      val errors: List[Error] = typeCheckErrors(
        """
        import golem.BaseAgent
        import golem.runtime.annotations.{agentDefinition, agentImplementation, readOnly}
        import golem.runtime.autowire.AgentImplementation
        import scala.concurrent.Future

        @agentDefinition("ephemeral-ro", mode = golem.runtime.annotations.DurabilityMode.Ephemeral)
        trait EphemeralReadOnlyAgent extends BaseAgent {
          class Id()
          @readOnly()
          def peek(name: String): Future[String]
        }

        @agentImplementation()
        final class EphemeralReadOnlyAgentImpl() extends EphemeralReadOnlyAgent {
          override def peek(name: String): Future[String] = Future.successful(name)
        }

        AgentImplementation.registerClass[EphemeralReadOnlyAgent, EphemeralReadOnlyAgentImpl]
        """
      )
      val combined = errors.map(_.message).mkString("\n")
      assertTrue(
        errors.nonEmpty,
        combined.contains("ephemeral") && combined.contains("@readOnly")
      )
    }
  )
}
