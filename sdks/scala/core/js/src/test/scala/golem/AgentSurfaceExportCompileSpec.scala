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

package golem

import golem.runtime.macros.AgentSurfaceExportMacro
import zio.test._

import scala.concurrent.Future

object AgentSurfaceExportCompileSpec extends ZIOSpecDefault {

  // ---------------------------------------------------------------------------
  // Test agent traits
  // ---------------------------------------------------------------------------

  @agentDefinition("unit-export")
  trait UnitExportAgent extends BaseAgent {
    class Id()
    def ping(): Future[String]
  }

  @agentDefinition("string-export")
  @description("An agent with a string constructor.")
  trait StringExportAgent extends BaseAgent {
    class Id(val value: String)
    def echo(): Future[String]
  }

  @agentDefinition("multi-export")
  trait MultiExportAgent extends BaseAgent {
    class Id(val host: String, val port: Int)
    def info(): Future[String]
  }

  @agentDefinition()
  trait DefaultNameAgent extends BaseAgent {
    class Id()
    def ping(): Future[String]
  }

  // ---------------------------------------------------------------------------
  // Tests
  // ---------------------------------------------------------------------------

  def spec = suite("AgentSurfaceExportCompileSpec")(
    test("export unit-constructor agent produces valid JSON") {
      val json = AgentSurfaceExportMacro.exportJson[UnitExportAgent]
      assertTrue(
        json.contains("\"typeName\":\"unit-export\""),
        json.contains("\"params\":[]"),
        json.contains("\"traitFqn\":")
      )
    },
    test("export single-param constructor includes param") {
      val json = AgentSurfaceExportMacro.exportJson[StringExportAgent]
      assertTrue(
        json.contains("\"typeName\":\"string-export\""),
        json.contains("\"name\":\"value\""),
        json.contains("\"description\":\"An agent with a string constructor.\"")
      )
    },
    test("export multi-param constructor includes all params") {
      val json = AgentSurfaceExportMacro.exportJson[MultiExportAgent]
      assertTrue(
        json.contains("\"typeName\":\"multi-export\""),
        json.contains("\"name\":\"host\""),
        json.contains("\"name\":\"port\"")
      )
    },
    test("export agent with default name uses trait name") {
      val json = AgentSurfaceExportMacro.exportJson[DefaultNameAgent]
      assertTrue(
        json.contains("\"typeName\":\"DefaultNameAgent\"")
      )
    },
    test("export JSON contains package/FQN info") {
      val json = AgentSurfaceExportMacro.exportJson[UnitExportAgent]
      assertTrue(
        json.contains("\"packageName\":"),
        json.contains("\"simpleName\":\"UnitExportAgent\""),
        json.contains("\"traitFqn\":")
      )
    },
    test("export JSON contains metadata fields") {
      val json = AgentSurfaceExportMacro.exportJson[UnitExportAgent]
      assertTrue(
        json.contains("\"mode\":\"durable\""),
        json.contains("\"snapshotting\":\"disabled\"")
      )
    },
    test("export agent without description has null") {
      val json = AgentSurfaceExportMacro.exportJson[UnitExportAgent]
      assertTrue(
        json.contains("\"description\":null")
      )
    }
  )
}
