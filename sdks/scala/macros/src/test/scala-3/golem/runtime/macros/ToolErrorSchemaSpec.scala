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

    @error(kind = "runtime", exitCode = 2)
    case RepositoryClean

    @error(kind = "usage", exitCode = 129)
    case BadAuthorFormat(author: String)

    @error(kind = "usage", exitCode = 128)
    case InvalidRevision(revision: String)

    @error(kind = "usage-error", exitCode = 130)
    case TooManyParents(count: Int)
  }

  private lazy val schema = ToolErrorSchemaDerivation.derive[CommitError]

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolErrorSchemaSpec")(
      test("error cases carry kind, exit code, doc and payload schema") {
        val cases = schema.errorCases.toOption.get
        assertTrue(
          cases.map(_.name) ==
            List("nothing-staged", "repository-clean", "bad-author-format", "invalid-revision", "too-many-parents"),
          cases.head.kind == ErrorKind.RuntimeError,
          cases.head.exitCode == 1,
          cases.head.doc.summary == "Nothing is staged.",
          cases.head.payload.isEmpty,
          cases(2).kind == ErrorKind.UsageError,
          cases(2).exitCode == 129,
          cases(2).payload == Some(IntoSchema[String].graph),
          cases(3).payload == Some(IntoSchema[String].graph),
          cases(4).payload == Some(IntoSchema[Int].graph)
        )
      },
      test("payload encoding") {
        assertTrue(
          schema.toErrorValue(CommitError.NothingStaged) ==
            Right(NamedToolError("nothing-staged", ToolErrorSupport.unitPayload)),
          schema.toErrorValue(CommitError.RepositoryClean) ==
            Right(NamedToolError("repository-clean", ToolErrorSupport.unitPayload)),
          schema.toErrorValue(CommitError.BadAuthorFormat("x")) ==
            Right(NamedToolError("bad-author-format", IntoSchema[String].toTyped("x"))),
          schema.toErrorValue(CommitError.InvalidRevision("abc")) ==
            Right(NamedToolError("invalid-revision", IntoSchema[String].toTyped("abc"))),
          schema.toErrorValue(CommitError.TooManyParents(3)) ==
            Right(NamedToolError("too-many-parents", IntoSchema[Int].toTyped(3)))
        )
      },
      test("decode by case name and validate its payload") {
        assertTrue(
          schema.fromErrorValue(NamedToolError("nothing-staged", ToolErrorSupport.unitPayload)) ==
            Right(CommitError.NothingStaged),
          schema.fromErrorValue(NamedToolError("repository-clean", ToolErrorSupport.unitPayload)) ==
            Right(CommitError.RepositoryClean),
          schema.fromErrorValue(NamedToolError("bad-author-format", IntoSchema[String].toTyped("bob"))) ==
            Right(CommitError.BadAuthorFormat("bob")),
          schema.fromErrorValue(NamedToolError("invalid-revision", IntoSchema[String].toTyped("abc"))) ==
            Right(CommitError.InvalidRevision("abc")),
          schema.fromErrorValue(NamedToolError("too-many-parents", IntoSchema[Int].toTyped(4))) ==
            Right(CommitError.TooManyParents(4)),
          schema.fromErrorValue(
            NamedToolError("bad-author-format", IntoSchema[Boolean].toTyped(true))
          ) == Left(ToolErrorSupport.unmatchedPayload)
        )
      }
    )
}
