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

package golem.runtime.macros

import scala.reflect.runtime.universe
import scala.tools.reflect.{ToolBox, ToolBoxError}
import zio.test._

object ReadOnlyEphemeralRejectSpec extends ZIOSpecDefault {

  def spec = suite("ReadOnlyEphemeralRejectSpec")(
    test("ephemeral agent with @readOnly method fails to compile (Scala 2)") {
      val toolbox = universe.runtimeMirror(getClass.getClassLoader).mkToolBox()
      val source =
        """
        import golem.runtime.annotations.{agentDefinition, readOnly, DurabilityMode}
        import golem.runtime.macros.AgentMacros
        import scala.concurrent.Future

        @agentDefinition("ephemeral-ro", mode = DurabilityMode.Ephemeral)
        trait EphemeralReadOnlyAgent {
          class Id()
          @readOnly()
          def peek(name: String): Future[String]
        }

        AgentMacros.agentMetadata[EphemeralReadOnlyAgent]
        """
      val error =
        try {
          toolbox.eval(toolbox.parse(source))
          None
        } catch {
          case e: ToolBoxError => Some(e.getMessage)
        }
      val message = error.getOrElse("")
      assertTrue(
        error.isDefined,
        message.contains("ephemeral") && message.contains("@readOnly")
      )
    }
  )
}
