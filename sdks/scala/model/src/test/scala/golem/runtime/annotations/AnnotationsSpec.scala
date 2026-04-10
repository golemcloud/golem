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

package golem.runtime.annotations

import zio.test._

object AnnotationsSpec extends ZIOSpecDefault {
  override def spec: Spec[TestEnvironment, Any] =
    suite("AnnotationsSpec")(
      test("DurabilityMode wire values and parsing") {
        assertTrue(DurabilityMode.Durable.wireValue() == "durable") &&
        assertTrue(DurabilityMode.Ephemeral.wireValue() == "ephemeral") &&
        assertTrue(DurabilityMode.Durable.toString == "durable") &&
        assertTrue(DurabilityMode.fromWireValue("durable") == Some(DurabilityMode.Durable)) &&
        assertTrue(DurabilityMode.fromWireValue("ephemeral") == Some(DurabilityMode.Ephemeral)) &&
        assertTrue(DurabilityMode.fromWireValue("unknown").isEmpty)
      },
      test("annotation classes can be constructed") {
        val a1 = new description("desc")
        val a2 = new prompt("prompt")
        val a3 = new agentImplementation()
        val a4 = new languageCode("en")
        val a5 = new mimeType("image/png")
        val a6 = new agentDefinition("MyAgent", DurabilityMode.Durable)
        val a7 = new agentDefinition()
        val a8 = new agentDefinition("Custom")

        assertTrue(
          a1.value == "desc",
          a2.value == "prompt",
          a3 != null,
          a4.value == "en",
          a5.value == "image/png",
          a6.typeName == "MyAgent",
          a6.mode == DurabilityMode.Durable,
          a7.typeName == "",
          a7.mode == DurabilityMode.Durable,
          a8.typeName == "Custom",
          a8.mode == DurabilityMode.Durable
        )
      }
    )
}
