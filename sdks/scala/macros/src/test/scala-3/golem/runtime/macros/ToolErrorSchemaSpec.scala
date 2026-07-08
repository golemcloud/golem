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

import golem.schema.{IntoSchema, SchemaValue}
import golem.tool.*
import golem.runtime.annotations.error
import zio.test.*

/**
 * Verifies the macro-derived [[ToolErrorSchema]]: error-case metadata, payload
 * encoding, and decode-by-payload-compatibility.
 */
object ToolErrorSchemaSpec extends ZIOSpecDefault {

  enum CommitError {

    /** Nothing is staged. */
    @error(kind = "runtime", exitCode = 1)
    case NothingStaged

    @error(kind = "usage", exitCode = 129)
    case BadAuthorFormat(author: String)

    @error(kind = "usage-error", exitCode = 130)
    case TooManyParents(count: Int)
  }

  private lazy val schema = ToolErrorSchemaDerivation.derive[CommitError]

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolErrorSchemaSpec")(
      test("error cases carry kind, exit code, doc and payload schema") {
        val cases = schema.errorCases.toOption.get
        assertTrue(
          cases.map(_.name) == List("nothing-staged", "bad-author-format", "too-many-parents"),
          cases.head.kind == ErrorKind.RuntimeError,
          cases.head.exitCode == 1,
          cases.head.doc.summary == "Nothing is staged.",
          cases.head.payload.isEmpty,
          cases(1).kind == ErrorKind.UsageError,
          cases(1).exitCode == 129,
          cases(1).payload == Some(IntoSchema[String].graph),
          cases(2).payload == Some(IntoSchema[Int].graph)
        )
      },
      test("payload encoding") {
        assertTrue(
          schema.toErrorPayloadValue(CommitError.NothingStaged) == Right(ToolErrorSupport.unitPayload),
          schema.toErrorPayloadValue(CommitError.BadAuthorFormat("x")) ==
            Right(IntoSchema[String].toTyped("x")),
          schema.toErrorPayloadValue(CommitError.TooManyParents(3)) ==
            Right(IntoSchema[Int].toTyped(3))
        )
      },
      test("decode by payload compatibility, in declaration order") {
        assertTrue(
          schema.fromErrorPayloadValue(ToolErrorSupport.unitPayload) ==
            Right(CommitError.NothingStaged),
          schema.fromErrorPayloadValue(IntoSchema[String].toTyped("bob")) ==
            Right(CommitError.BadAuthorFormat("bob")),
          schema.fromErrorPayloadValue(IntoSchema[Int].toTyped(4)) ==
            Right(CommitError.TooManyParents(4)),
          schema.fromErrorPayloadValue(
            golem.schema.TypedSchemaValue(IntoSchema[Boolean].graph, SchemaValue.BoolValue(true))
          ) == Left(ToolErrorSupport.unmatchedPayload)
        )
      }
    )
}
