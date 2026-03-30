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

import zio.test._

object PackageExportsCompileSpec extends ZIOSpecDefault {

  def spec = suite("PackageExportsCompileSpec")(
    test("DurabilityMode.wireValue returns correct strings") {
      assertTrue(
        golem.DurabilityMode.Durable.wireValue() == "durable",
        golem.DurabilityMode.Ephemeral.wireValue() == "ephemeral"
      )
    },
    test("DurabilityMode.fromWireValue parses durable") {
      assertTrue(golem.DurabilityMode.fromWireValue("durable").contains(golem.DurabilityMode.Durable))
    },
    test("DurabilityMode.fromWireValue parses ephemeral") {
      assertTrue(golem.DurabilityMode.fromWireValue("ephemeral").contains(golem.DurabilityMode.Ephemeral))
    },
    test("DurabilityMode.fromWireValue is case-insensitive") {
      assertTrue(
        golem.DurabilityMode.fromWireValue("DURABLE").contains(golem.DurabilityMode.Durable),
        golem.DurabilityMode.fromWireValue("Ephemeral").contains(golem.DurabilityMode.Ephemeral)
      )
    },
    test("DurabilityMode.fromWireValue returns None for unknown") {
      assertTrue(
        golem.DurabilityMode.fromWireValue("unknown").isEmpty,
        golem.DurabilityMode.fromWireValue("").isEmpty,
        golem.DurabilityMode.fromWireValue(null).isEmpty
      )
    },
    test("DurabilityMode.toString matches wireValue") {
      assertTrue(
        golem.DurabilityMode.Durable.toString == "durable",
        golem.DurabilityMode.Ephemeral.toString == "ephemeral"
      )
    }
  )
}
