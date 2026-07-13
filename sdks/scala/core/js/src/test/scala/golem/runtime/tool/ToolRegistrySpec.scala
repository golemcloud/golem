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

package golem.runtime.tool

import golem.tool.ExtendedToolType
import zio.test._

import scala.concurrent.Future

object ToolRegistrySpec extends ZIOSpecDefault {
  import ToolTestFixtures._

  private val noopInvoker: ToolRegistry.ToolInvoker =
    (_, input, _, _) => Future.successful(Right(ToolInvocationResult(Some(input), None)))

  private def registrationError(tool: ExtendedToolType): Option[Throwable] =
    try {
      ToolRegistry.register(tool)
      None
    } catch {
      case t: Throwable => Some(t)
    }

  def spec: Spec[Any, Any] = suite("ToolRegistrySpec")(
    test("duplicate_registration_fails") {
      ToolRegistry.register(leafTool("reg-dupe"))
      val err = registrationError(leafTool("reg-dupe"))
      assertTrue(
        err.exists(_.isInstanceOf[IllegalArgumentException]),
        err.exists(_.getMessage.contains("reg-dupe"))
      )
    },
    test("invalid_descriptor_registration_fails") {
      // Uppercase command names violate the identifier grammar, so the
      // registration-time `tryToTool` validation must reject the tool.
      val err = registrationError(leafTool("Reg-Invalid"))
      assertTrue(
        err.exists(_.isInstanceOf[IllegalArgumentException]),
        err.exists(_.getMessage.contains("tool descriptor build failed")),
        ToolRegistry.getTool("Reg-Invalid").isEmpty
      )
    },
    test("registered_tool_is_retrievable_by_name") {
      val tool = richTool("reg-rich")
      ToolRegistry.register(tool)
      assertTrue(
        ToolRegistry.getTool("reg-rich").contains(tool.toTool),
        ToolRegistry.getExtendedTool("reg-rich").contains(tool),
        ToolRegistry.getInvoker("reg-rich").isEmpty,
        ToolRegistry.getTool("reg-unknown").isEmpty
      )
    },
    test("registered_invoker_is_retrievable_by_name") {
      ToolRegistry.registerInvoker(leafTool("reg-invocable"), noopInvoker)
      assertTrue(ToolRegistry.getInvoker("reg-invocable").isDefined)
    },
    test("all_tools_is_sorted_by_name") {
      ToolRegistry.register(leafTool("reg-zz-last"))
      ToolRegistry.register(leafTool("reg-aa-first"))
      val names = ToolRegistry.allTools.map(_.commands.nodes.head.name)
      assertTrue(
        names.contains("reg-aa-first"),
        names.contains("reg-zz-last"),
        names == names.sorted,
        names.indexOf("reg-aa-first") < names.indexOf("reg-zz-last")
      )
    }
  )
}
