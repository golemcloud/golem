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

package golem.tool

import golem.schema.{IntoSchema, TypedSchemaValue}
import zio.test._

object ToolInvokeErrorSpec extends ZIOSpecDefault {

  private val payload: TypedSchemaValue =
    implicitly[IntoSchema[String]].toTyped("tool failure")

  def spec: Spec[Any, Any] = suite("ToolInvokeErrorSpec")(
    test("all wire variants round-trip through the generic invocation error") {
      val errors: List[ToolInvokeError[TypedSchemaValue]] = List(
        ToolInvokeError.InvalidToolName("missing"),
        ToolInvokeError.InvalidCommandPath(List("remote", "add")),
        ToolInvokeError.InvalidInput("bad input"),
        ToolInvokeError.ConstraintViolation("denied"),
        ToolInvokeError.InvalidResult("bad result"),
        ToolInvokeError.Tool(payload)
      )

      assertTrue(errors.forall(error => ToolInvokeError.fromWire(ToolInvokeError.toWire(error)) == error))
    },
    test("mapTool transforms only the declared tool error") {
      val protocolErrors: List[ToolInvokeError[String]] = List(
        ToolInvokeError.InvalidToolName("missing"),
        ToolInvokeError.InvalidCommandPath(List("run")),
        ToolInvokeError.InvalidInput("bad input"),
        ToolInvokeError.ConstraintViolation("denied"),
        ToolInvokeError.InvalidResult("bad result")
      )

      assertTrue(
        protocolErrors.map(_.mapTool(_.length)) == protocolErrors,
        ToolInvokeError.Tool("failure").mapTool(_.length) == ToolInvokeError.Tool(7)
      )
    },
    test("underlying misuse remains outside the wire error algebra") {
      val overlapping = new ToolUnderlyingMisuseException(ToolUnderlyingMisuse.OverlappingInvocation)
      val revoked     = new ToolUnderlyingMisuseException(ToolUnderlyingMisuse.Revoked)

      assertTrue(
        overlapping.reason == ToolUnderlyingMisuse.OverlappingInvocation,
        overlapping.getMessage.contains("already in flight"),
        revoked.reason == ToolUnderlyingMisuse.Revoked,
        revoked.getMessage.contains("no longer available")
      )
    }
  )
}
