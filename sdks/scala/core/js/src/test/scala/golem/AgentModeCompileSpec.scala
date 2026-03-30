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

import golem.runtime.autowire.AgentMode
import zio.test._

object AgentModeCompileSpec extends ZIOSpecDefault {

  def spec = suite("AgentModeCompileSpec")(
    test("AgentMode.Durable has value 'durable'") {
      assertTrue(AgentMode.Durable.value == "durable")
    },
    test("AgentMode.Ephemeral has value 'ephemeral'") {
      assertTrue(AgentMode.Ephemeral.value == "ephemeral")
    },
    test("AgentMode.fromString parses 'durable'") {
      assertTrue(AgentMode.fromString("durable").contains(AgentMode.Durable))
    },
    test("AgentMode.fromString parses 'ephemeral'") {
      assertTrue(AgentMode.fromString("ephemeral").contains(AgentMode.Ephemeral))
    },
    test("AgentMode.fromString is case-insensitive") {
      assertTrue(
        AgentMode.fromString("DURABLE").contains(AgentMode.Durable),
        AgentMode.fromString("Ephemeral").contains(AgentMode.Ephemeral),
        AgentMode.fromString("EPHEMERAL").contains(AgentMode.Ephemeral)
      )
    },
    test("AgentMode.fromString returns None for unknown values") {
      assertTrue(
        AgentMode.fromString("unknown").isEmpty,
        AgentMode.fromString("").isEmpty
      )
    },
    test("AgentMode.fromString returns None for null") {
      assertTrue(AgentMode.fromString(null).isEmpty)
    },
    test("AgentMode sealed trait is exhaustive") {
      def describe(mode: AgentMode): String = mode match {
        case AgentMode.Durable   => "durable"
        case AgentMode.Ephemeral => "ephemeral"
      }
      assertTrue(
        describe(AgentMode.Durable) == "durable",
        describe(AgentMode.Ephemeral) == "ephemeral"
      )
    }
  )

}
