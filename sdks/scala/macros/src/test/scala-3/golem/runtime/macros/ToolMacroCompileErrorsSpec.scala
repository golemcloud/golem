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
 * Verifies the compile-time authoring errors of the tool macros, mirroring the
 * Rust SDK's UI-fail cases plus the attribute-grammar validations.
 */
object ToolMacroCompileErrorsSpec extends ZIOSpecDefault {

  private inline def errorsOf(inline code: String): List[String] =
    typeCheckErrors(code).map(_.message)

  private inline val prelude =
    """
    import golem.runtime.annotations.*
    import golem.runtime.macros.ToolDefinitionMacro
    """

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolMacroCompileErrorsSpec")(
      test("@arg referring to an unknown parameter") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Grep {
            @arg("missing", scope = "option")
            def grep(pattern: String): Unit
          }
          ToolDefinitionMacro.metadata[Grep]
          """
        )
        assertTrue(
          errors.exists(
            _.contains("@arg(...) refers to unknown parameter `missing`; the method has no such parameter")
          )
        )
      },
      test("implicit-body method name divergence") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Grep {
            @command(name = "search")
            def grep(pattern: String): Unit
          }
          ToolDefinitionMacro.metadata[Grep]
          """
        )
        assertTrue(
          errors.exists(
            _.contains(
              "the implicit-body method's @command(name = \"search\") diverges from the tool name " +
                "\"grep\"; the root command name must equal the tool name (§5.8.1)"
            )
          )
        )
      },
      test("@result on a subtree method") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Rem {
            def list(): Unit
          }
          @toolDefinition(version = "1.0.0")
          trait Git {
            @result(formatters = Array("json"))
            def rem(): Rem
          }
          ToolDefinitionMacro.metadata[Git]
          """
        )
        assertTrue(
          errors.exists(_.contains("@constraint / @result are not supported on a subtree method"))
        )
      },
      test("Option flag parameter") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Run {
            @arg("verbose", kind = "flag")
            def run(verbose: Option[Boolean]): Unit
          }
          ToolDefinitionMacro.metadata[Run]
          """
        )
        assertTrue(errors.exists(_.contains("a flag parameter must not be `Option[_]`")))
      },
      test("count flag parameter must be Int") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Run {
            @arg("verbose", kind = "count-flag")
            def run(verbose: String): Unit
          }
          ToolDefinitionMacro.metadata[Run]
          """
        )
        assertTrue(errors.exists(_.contains("a count flag parameter must be `Int`")))
      },
      test("at most one tail positional") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Run {
            @arg("first", scope = "tail")
            @arg("second", scope = "tail")
            def run(first: Seq[String], second: Seq[String]): Unit
          }
          ToolDefinitionMacro.metadata[Run]
          """
        )
        assertTrue(errors.exists(_.contains("a command may have at most one tail positional")))
      },
      test("invalid arg scope value") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Run {
            @arg("input", scope = "bogus")
            def run(input: String): Unit
          }
          ToolDefinitionMacro.metadata[Run]
          """
        )
        assertTrue(
          errors.exists(
            _.contains("invalid arg scope `bogus`; expected one of: global, positional, option, flag, tail")
          )
        )
      },
      test("duplicate @arg for one parameter") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Run {
            @arg("input", doc = "a")
            @arg("input", doc = "b")
            def run(input: String): Unit
          }
          ToolDefinitionMacro.metadata[Run]
          """
        )
        assertTrue(errors.exists(_.contains("duplicate @arg(...) for parameter `input`")))
      },
      test("@arg on an auto-injected Principal parameter") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Run {
            @arg("principal", doc = "who")
            def run(input: String, principal: golem.Principal): Unit
          }
          ToolDefinitionMacro.metadata[Run]
          """
        )
        assertTrue(
          errors.exists(
            _.contains(
              "auto-injected Principal parameters cannot have @arg annotations because they are " +
                "not part of the tool input schema"
            )
          )
        )
      },
      test("multiple implicit-body methods") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait RunIt {
            def runIt(a: String): Unit
            def run_it(b: String): Unit
          }
          ToolDefinitionMacro.metadata[RunIt]
          """
        )
        assertTrue(
          errors.exists(
            _.contains(
              "multiple methods map to the tool's root command name `run-it`; only one method may " +
                "be the implicit-body handler (§5.8.1)"
            )
          )
        )
      },
      test("command name colliding with the root name") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Tools {
            def tools(a: String): Unit
            @command(name = "tools")
            def other(b: String): Unit
          }
          ToolDefinitionMacro.metadata[Tools]
          """
        )
        assertTrue(
          errors.exists(
            _.contains(
              "command `tools` collides with the tool's root command name; rename the method or " +
                "use @command(name = ...)"
            )
          )
        )
      },
      test("global positional is rejected") {
        val errors = errorsOf(
          prelude + """
          @toolDefinition(version = "1.0.0")
          trait Run {
            @arg("input", scope = "global", kind = "flag")
            def run(input: Seq[String]): Unit
          }
          ToolDefinitionMacro.metadata[Run]
          """
        )
        assertTrue(errors.nonEmpty)
      },
      test("stateful tool implementation classes are rejected") {
        val errors = errorsOf(
          prelude + """
          import golem.runtime.macros.ToolImplementationMacro
          @toolDefinition(version = "1.0.0")
          trait Run {
            def run(input: String): Unit
          }
          final class RunImpl(state: String) extends Run {
            def run(input: String): Unit = ()
          }
          ToolImplementationMacro.handle[Run, RunImpl]
          """
        )
        assertTrue(
          errors.exists(_.contains("a tool implementation class must have an empty primary constructor"))
        )
      },
      test("stateful tool middleware classes are rejected") {
        val errors = errorsOf(
          prelude + """
          import golem.runtime.macros.ToolMiddlewareMacro
          import golem.tool.*
          import scala.concurrent.Future
          @toolDefinition(version = "1.0.0")
          trait Run { def run(input: String): String }
          trait RunUnderlying
          trait RunMiddleware extends RunMiddleware.Adapter[RunUnderlying]
          object RunMiddleware {
            trait Adapter[U] {
              def run(underlying: U, input: String): Future[Either[ToolInvokeError[Nothing], String]]
            }
          }
          final class RunPolicy(state: String) extends RunMiddleware {
            def run(
              underlying: RunUnderlying,
              input: String
            ): Future[Either[ToolInvokeError[Nothing], String]] = Future.successful(Right(input))
          }
          ToolMiddlewareMacro.transparentHandle[Run, RunUnderlying, RunMiddleware, RunPolicy](_ => null)
          """
        )
        assertTrue(
          errors.exists(_.contains("RunPolicy must have an empty primary constructor"))
        )
      },
      test("generic tool middleware classes are rejected") {
        val errors = errorsOf(
          prelude + """
          import golem.runtime.macros.ToolMiddlewareMacro
          import golem.tool.*
          import scala.concurrent.Future
          @toolDefinition(version = "1.0.0")
          trait Run { def run(input: String): String }
          trait RunUnderlying
          trait RunMiddleware extends RunMiddleware.Adapter[RunUnderlying]
          object RunMiddleware {
            trait Adapter[U] {
              def run(underlying: U, input: String): Future[Either[ToolInvokeError[Nothing], String]]
            }
          }
          final class RunPolicy[A] extends RunMiddleware {
            def run(
              underlying: RunUnderlying,
              input: String
            ): Future[Either[ToolInvokeError[Nothing], String]] = Future.successful(Right(input))
          }
          ToolMiddlewareMacro.transparentHandle[Run, RunUnderlying, RunMiddleware, RunPolicy[String]](_ => null)
          """
        )
        assertTrue(
          errors.exists(_.contains("tool middleware implementation must not have type parameters"))
        )
      },
      test("middleware registration rejects a prefix-matching impostor surface") {
        val errors = errorsOf(
          prelude + """
          import golem.runtime.macros.ToolMiddlewareMacro
          import golem.tool.*
          import scala.concurrent.Future
          @toolDefinition(version = "1.0.0")
          trait Run { def run(input: String): String }
          trait RunUnderlying
          trait RunMiddleware extends RunMiddleware.Adapter[RunUnderlying]
          object RunMiddleware {
            trait Adapter[U] {
              def run(underlying: U, input: String): Future[Either[ToolInvokeError[Nothing], String]]
            }
          }
          trait RunMiddlewareFake {
            def run(
              underlying: RunUnderlying,
              input: String
            ): Future[Either[ToolInvokeError[Nothing], String]]
          }
          final class RunPolicy extends RunMiddlewareFake {
            def run(
              underlying: RunUnderlying,
              input: String
            ): Future[Either[ToolInvokeError[Nothing], String]] = Future.successful(Right(input))
          }
          ToolMiddlewareMacro.transparentHandle[Run, RunUnderlying, RunMiddlewareFake, RunPolicy](_ => null)
          """
        )
        assertTrue(errors.exists(_.contains("expected generated middleware surface")))
      },
      test("adapter middleware expected type must match its generated underlying") {
        val errors = errorsOf(
          prelude + """
          import golem.runtime.macros.ToolMiddlewareMacro
          import golem.tool.*
          import scala.concurrent.Future
          @toolDefinition(version = "1.0.0")
          trait Run { def run(input: String): String }
          @toolDefinition(version = "1.0.0")
          trait Backend { def execute(input: String): String }
          trait BackendUnderlying
          object RunMiddleware {
            trait Adapter[U] {
              def run(underlying: U, input: String): Future[Either[ToolInvokeError[Nothing], String]]
            }
          }
          final class RunPolicy extends RunMiddleware.Adapter[BackendUnderlying] {
            def run(
              underlying: BackendUnderlying,
              input: String
            ): Future[Either[ToolInvokeError[Nothing], String]] = Future.successful(Right(input))
          }
          ToolMiddlewareMacro.adapterHandle[
            Run,
            Run,
            BackendUnderlying,
            RunMiddleware.Adapter[BackendUnderlying],
            RunPolicy
          ](_ => null)
          """
        )
        assertTrue(
          errors.exists(message =>
            message.contains("expected generated underlying type") && message.contains("RunUnderlying")
          )
        )
      },
      test("stateful universal tool middleware classes are rejected") {
        val errors = errorsOf(
          prelude + """
          import golem.runtime.macros.ToolMiddlewareMacro
          import golem.schema.TypedSchemaValue
          import golem.tool.*
          import scala.concurrent.Future
          @universalToolMiddleware(name = "universal")
          final class UniversalPolicy(state: String) extends UniversalToolMiddleware {
            def invoke(
              invocation: UniversalToolMiddlewareInvocation,
              underlying: UniversalToolUnderlying
            ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] =
              underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
          }
          ToolMiddlewareMacro.universalHandle[UniversalPolicy]
          """
        )
        assertTrue(
          errors.exists(_.contains("UniversalPolicy must have an empty primary constructor"))
        )
      },
      test("universal tool middleware classes must directly extend the universal surface") {
        val errors = errorsOf(
          prelude + """
          import golem.runtime.macros.ToolMiddlewareMacro
          import golem.schema.TypedSchemaValue
          import golem.tool.*
          import scala.concurrent.Future
          abstract class StatefulUniversalPolicy(val state: String) extends UniversalToolMiddleware {
            def invoke(
              invocation: UniversalToolMiddlewareInvocation,
              underlying: UniversalToolUnderlying
            ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] =
              underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
          }
          @universalToolMiddleware(name = "universal")
          final class UniversalPolicy extends StatefulUniversalPolicy("state")
          ToolMiddlewareMacro.universalHandle[UniversalPolicy]
          """
        )
        assertTrue(
          errors.exists(_.contains("UniversalPolicy must directly extend golem.tool.UniversalToolMiddleware"))
        )
      },
      test("generic universal tool middleware classes are rejected") {
        val errors = errorsOf(
          prelude + """
          import golem.runtime.macros.ToolMiddlewareMacro
          import golem.schema.TypedSchemaValue
          import golem.tool.*
          import scala.concurrent.Future
          @universalToolMiddleware(name = "universal")
          final class UniversalPolicy[A] extends UniversalToolMiddleware {
            def invoke(
              invocation: UniversalToolMiddlewareInvocation,
              underlying: UniversalToolUnderlying
            ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] =
              underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
          }
          ToolMiddlewareMacro.universalHandle[UniversalPolicy[String]]
          """
        )
        assertTrue(
          errors.exists(_.contains("tool middleware implementation must not have type parameters"))
        )
      },
      test("universal tool middleware classes require their annotation") {
        val errors = errorsOf(
          prelude + """
          import golem.runtime.macros.ToolMiddlewareMacro
          import golem.schema.TypedSchemaValue
          import golem.tool.*
          import scala.concurrent.Future
          final class UniversalPolicy extends UniversalToolMiddleware {
            def invoke(
              invocation: UniversalToolMiddlewareInvocation,
              underlying: UniversalToolUnderlying
            ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] =
              underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
          }
          ToolMiddlewareMacro.universalHandle[UniversalPolicy]
          """
        )
        assertTrue(
          errors.exists(_.contains("must have exactly one @universalToolMiddleware annotation"))
        )
      }
    )
}
