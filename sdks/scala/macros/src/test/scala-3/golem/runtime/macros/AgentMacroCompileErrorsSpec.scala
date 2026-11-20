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

import zio.test.*

import scala.compiletime.testing.typeCheckErrors

/**
 * Rule 8: a `Principal`, a `Caller` and a `TokenProvenance` are live values,
 * never data. Every position that would carry one as data fails the build with
 * the same message.
 */
object AgentMacroCompileErrorsSpec extends ZIOSpecDefault {

  private inline def errorsOf(inline code: String): List[String] =
    typeCheckErrors(code).map(_.message)

  private inline val prelude =
    """
    import golem.runtime.annotations.*
    import golem.runtime.macros.AgentDefinitionMacro
    import scala.concurrent.Future
    """

  private val message =
    "Principal and Caller cannot appear in a method, constructor or tool signature, or inside a value type."

  override def spec: Spec[TestEnvironment, Any] =
    suite("AgentMacroCompileErrorsSpec")(
      test("a Principal cannot be a method parameter") {
        val errors = errorsOf(
          prelude + """
          @agentDefinition()
          trait Bad {
            class Id()
            def run(who: golem.Principal): Future[String]
          }
          AgentDefinitionMacro.generate[Bad]
          """
        )
        assertTrue(errors.exists(_.contains(message)))
      },
      test("a Caller cannot be a method parameter") {
        val errors = errorsOf(
          prelude + """
          @agentDefinition()
          trait Bad {
            class Id()
            def run(who: golem.Caller): Future[String]
          }
          AgentDefinitionMacro.generate[Bad]
          """
        )
        assertTrue(errors.exists(_.contains(message)))
      },
      test("a TokenProvenance cannot be nested in an input") {
        val errors = errorsOf(
          prelude + """
          @agentDefinition()
          trait Bad {
            class Id()
            def run(tokens: List[golem.TokenProvenance]): Future[String]
          }
          AgentDefinitionMacro.generate[Bad]
          """
        )
        assertTrue(errors.exists(_.contains(message)))
      },
      test("a Principal cannot be a result") {
        val errors = errorsOf(
          prelude + """
          @agentDefinition()
          trait Bad {
            class Id()
            def run(): Future[golem.Principal]
          }
          AgentDefinitionMacro.generate[Bad]
          """
        )
        assertTrue(errors.exists(_.contains(message)))
      },
      test("a Principal cannot be part of the agent id") {
        val errors = errorsOf(
          prelude + """
          @agentDefinition()
          trait Bad {
            class Id(val who: golem.Principal)
            def run(): Future[String]
          }
          AgentDefinitionMacro.generate[Bad]
          """
        )
        assertTrue(errors.exists(_.contains(message)))
      },
      test("an InvocationContext and a PrincipalRef are accepted") {
        val errors = errorsOf(
          prelude + """
          @agentDefinition()
          trait Ok {
            class Id(val owner: golem.PrincipalRef)
            def run(context: golem.InvocationContext, by: golem.PrincipalRef): Future[golem.PrincipalRef]
          }
          AgentDefinitionMacro.generate[Ok]
          """
        )
        assertTrue(errors.isEmpty)
      }
    )
}
