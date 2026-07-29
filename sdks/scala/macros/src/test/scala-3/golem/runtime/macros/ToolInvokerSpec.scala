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

import golem.Principal
import golem.schema.{IntoSchema, SchemaValue, TypedSchemaValue}
import golem.tool.*
import golem.runtime.annotations.*
import zio.test.*

import scala.concurrent.Future

/**
 * Verifies the macro-generated tool invocation surface on the JVM using a fake
 * invocation environment: canonical input decoding, Principal injection,
 * sync/async result encoding, custom-error encoding, and subtree forwarding.
 */
object ToolInvokerSpec extends ZIOSpecDefault {

  enum EchoError {
    @error(kind = "usage", exitCode = 2)
    case BadInput(message: String)
  }

  @toolDefinition(version = "1.0.0")
  trait Echo {
    def echo(input: String): String

    def fail(message: String): Either[EchoError, String]

    def asyncEcho(input: String): Future[String]

    def whoami(input: String, principal: Principal): String

    @arg("times", kind = "count-flag", max = 5)
    def repeat(input: String, times: Int): String

    def slurp(stdin: ToolInputStream): String
  }

  final class EchoImpl extends Echo {
    def echo(input: String): String = s"echo: $input"

    def fail(message: String): Either[EchoError, String] = Left(EchoError.BadInput(message))

    def asyncEcho(input: String): Future[String] = Future.successful(s"async: $input")

    def whoami(input: String, principal: Principal): String = s"$input/$principal"

    def repeat(input: String, times: Int): String = input * times

    def slurp(stdin: ToolInputStream): String = stdin match {
      case FakeStdin(content) => content
      case _                  => "?"
    }
  }

  private final case class FakeStdin(content: String) extends ToolInputStream

  @toolDefinition(version = "0.1.0")
  trait Remote {
    def add(name: String, url: String): String
  }

  final class RemoteImpl extends Remote {
    def add(name: String, url: String): String = s"$name=$url"
  }

  @toolDefinition(version = "0.2.0")
  trait Git {
    def remote(gitDir: String): Remote
  }

  final class GitImpl extends Git {
    def remote(gitDir: String): Remote = new RemoteImpl
  }

  private final class FakeEnv(
    tools: Map[String, (ExtendedToolType, ToolInvokeHandler)] = Map.empty
  ) extends ToolInvokeEnv {
    def stdout(): ToolOutputStream =
      throw new UnsupportedOperationException("no stdout in tests")
    def invokerFor(toolName: String): Option[ToolInvokeHandler]     = tools.get(toolName).map(_._2)
    def extendedToolFor(toolName: String): Option[ExtendedToolType] = tools.get(toolName).map(_._1)
  }

  private def outcome[A](f: Future[A]): A =
    f.value.getOrElse(throw new IllegalStateException("future did not complete synchronously")).get

  private def input(tool: ExtendedToolType, path: List[String], values: SchemaValue*): TypedSchemaValue = {
    val idx    = tool.commandIndexByPath(path).getOrElse(sys.error(s"no command at $path"))
    val schema = tool.canonicalInputRecordSchema(idx).toOption.get
    TypedSchemaValue(schema, SchemaValue.RecordValue(values.toList))
  }

  private lazy val echoHandle = ToolImplementationMacro.handle[Echo, EchoImpl]
  private lazy val echoTool   = {
    val ctx = new ToolBuildCtx
    echoHandle.descriptor(ctx).toOption.get
  }
  private lazy val echoHandler =
    ToolInvokerRuntime.handler(echoTool, echoHandle, new FakeEnv())

  private lazy val remoteHandle  = ToolImplementationMacro.handle[Remote, RemoteImpl]
  private lazy val remoteTool    = remoteHandle.descriptor(new ToolBuildCtx).toOption.get
  private lazy val remoteHandler =
    ToolInvokerRuntime.handler(remoteTool, remoteHandle, new FakeEnv())

  private lazy val gitHandle  = ToolImplementationMacro.handle[Git, GitImpl]
  private lazy val gitTool    = gitHandle.descriptor(new ToolBuildCtx).toOption.get
  private lazy val gitEnv     = new FakeEnv(Map("remote" -> ((remoteTool, remoteHandler))))
  private lazy val gitHandler =
    ToolInvokerRuntime.handler(gitTool, gitHandle, gitEnv)

  private val anonymous = Principal.Anonymous

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolInvokerSpec")(
      test("root command invocation encodes the successful result") {
        val result = outcome(
          echoHandler.invoke(Nil, input(echoTool, Nil, SchemaValue.StringValue("hi")), None, anonymous)
        )
        assertTrue(result == Right(ToolInvokeResult(Some(IntoSchema[String].toTyped("echo: hi")), None)))
      },
      test("async methods are awaited") {
        val result = outcome(
          echoHandler.invoke(
            List("async-echo"),
            input(echoTool, List("async-echo"), SchemaValue.StringValue("hi")),
            None,
            anonymous
          )
        )
        assertTrue(result == Right(ToolInvokeResult(Some(IntoSchema[String].toTyped("async: hi")), None)))
      },
      test("declared errors become custom-error payloads") {
        val result = outcome(
          echoHandler.invoke(
            List("fail"),
            input(echoTool, List("fail"), SchemaValue.StringValue("nope")),
            None,
            anonymous
          )
        )
        assertTrue(result == Left(ToolInvokeError.Custom(IntoSchema[String].toTyped("nope"))))
      },
      test("Principal parameters are injected and excluded from the schema") {
        val idx    = echoTool.commandIndexByPath(List("whoami")).get
        val fields = echoTool.canonicalInputFields(idx).map(_.name)
        val result = outcome(
          echoHandler.invoke(
            List("whoami"),
            input(echoTool, List("whoami"), SchemaValue.StringValue("me")),
            None,
            anonymous
          )
        )
        assertTrue(
          fields == List("input"),
          result == Right(ToolInvokeResult(Some(IntoSchema[String].toTyped("me/Anonymous")), None))
        )
      },
      test("count flags decode from their u32 canonical field") {
        val result = outcome(
          echoHandler.invoke(
            List("repeat"),
            input(
              echoTool,
              List("repeat"),
              SchemaValue.StringValue("ab"),
              SchemaValue.U32Value(3L)
            ),
            None,
            anonymous
          )
        )
        assertTrue(result == Right(ToolInvokeResult(Some(IntoSchema[String].toTyped("ababab")), None)))
      },
      test("declared stdin streams are injected and excluded from the schema") {
        val idx    = echoTool.commandIndexByPath(List("slurp")).get
        val fields = echoTool.canonicalInputFields(idx).map(_.name)
        val stream = echoTool.commands(idx).body.get.stdin
        val ok     = outcome(
          echoHandler.invoke(
            List("slurp"),
            input(echoTool, List("slurp")),
            Some(FakeStdin("piped")),
            anonymous
          )
        )
        val missing = outcome(
          echoHandler.invoke(List("slurp"), input(echoTool, List("slurp")), None, anonymous)
        )
        assertTrue(
          fields.isEmpty,
          stream.isDefined,
          ok == Right(ToolInvokeResult(Some(IntoSchema[String].toTyped("piped")), None)),
          missing == Left(
            ToolInvokeError.InvalidInput("tool invocation did not contain declared stdin stream")
          )
        )
      },
      test("unknown command paths are rejected") {
        val result = outcome(
          echoHandler.invoke(
            List("bogus"),
            IntoSchema[String].toTyped("x"),
            None,
            anonymous
          )
        )
        assertTrue(result == Left(ToolInvokeError.InvalidCommandPath(List("bogus"))))
      },
      test("field count mismatches are rejected as invalid input") {
        val result = outcome(
          echoHandler.invoke(
            Nil,
            TypedSchemaValue(
              echoTool.canonicalInputRecordSchema(0).toOption.get,
              SchemaValue.RecordValue(Nil)
            ),
            None,
            anonymous
          )
        )
        assertTrue(
          result == Left(
            ToolInvokeError.InvalidInput("tool input record has 0 fields, expected 1 canonical fields")
          )
        )
      },
      suite("subtree forwarding")(
        test("subtree paths forward to the registered child tool") {
          val result = outcome(
            gitHandler.invoke(
              List("remote", "add"),
              input(
                gitTool,
                List("remote", "add"),
                SchemaValue.StringValue(".git"),
                SchemaValue.StringValue("origin"),
                SchemaValue.StringValue("https://example.com")
              ),
              None,
              anonymous
            )
          )
          assertTrue(
            result == Right(
              ToolInvokeResult(Some(IntoSchema[String].toTyped("origin=https://example.com")), None)
            )
          )
        },
        test("an unregistered child tool is an invalid tool name") {
          val emptyEnvHandler = ToolInvokerRuntime.handler(gitTool, gitHandle, new FakeEnv())
          val result          = outcome(
            emptyEnvHandler.invoke(
              List("remote", "add"),
              input(
                gitTool,
                List("remote", "add"),
                SchemaValue.StringValue(".git"),
                SchemaValue.StringValue("origin"),
                SchemaValue.StringValue("u")
              ),
              None,
              anonymous
            )
          )
          assertTrue(result == Left(ToolInvokeError.InvalidToolName("remote")))
        }
      )
    )
}
