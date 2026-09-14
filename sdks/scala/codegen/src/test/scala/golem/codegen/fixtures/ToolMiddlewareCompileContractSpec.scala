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

package golem.codegen.fixtures

import scala.annotation.StaticAnnotation
import scala.collection.mutable
import scala.concurrent.duration._
import scala.concurrent.{Await, ExecutionContext, Future}

object ToolMiddlewareCompileContract {

  object annotations {
    final class toolMiddleware(
      val name: String,
      val aliases: Array[String] = Array.empty
    ) extends StaticAnnotation

    final class universalToolMiddleware(
      val name: String,
      val aliases: Array[String] = Array.empty
    ) extends StaticAnnotation
  }

  import annotations._

  sealed trait ToolInvokeError[+E] {
    def mapTool[E2](f: E => E2): ToolInvokeError[E2] =
      this match {
        case ToolInvokeError.Tool(error)                => ToolInvokeError.Tool(f(error))
        case error: ToolInvokeError.InvalidToolName     => error
        case error: ToolInvokeError.InvalidCommandPath  => error
        case error: ToolInvokeError.InvalidInput        => error
        case error: ToolInvokeError.ConstraintViolation => error
        case error: ToolInvokeError.InvalidResult       => error
      }
  }

  object ToolInvokeError {
    final case class InvalidToolName(name: String)          extends ToolInvokeError[Nothing]
    final case class InvalidCommandPath(path: List[String]) extends ToolInvokeError[Nothing]
    final case class InvalidInput(message: String)          extends ToolInvokeError[Nothing]
    final case class ConstraintViolation(message: String)   extends ToolInvokeError[Nothing]
    final case class InvalidResult(message: String)         extends ToolInvokeError[Nothing]
    final case class Tool[E](error: E)                      extends ToolInvokeError[E]
  }

  final case class TypedValue(value: String)
  final case class ToolMetadata(name: String, version: String)

  trait ToolMiddlewareInputHandle
  trait ToolMiddlewareOutputHandle

  final case class ToolMiddlewareResult(
    result: Option[TypedValue],
    stdout: Option[ToolMiddlewareOutputHandle]
  )

  final case class RawInvocation(
    commandPath: List[String],
    input: TypedValue,
    stdin: Option[ToolMiddlewareInputHandle]
  )

  trait RawToolUnderlying {
    def invoke(
      commandPath: List[String],
      input: TypedValue,
      stdin: Option[ToolMiddlewareInputHandle]
    ): Future[Either[ToolInvokeError[TypedValue], ToolMiddlewareResult]]
  }

  trait UniversalToolUnderlying extends RawToolUnderlying

  final case class UniversalToolMiddlewareInvocation(
    toolName: String,
    toolMetadata: ToolMetadata,
    commandPath: List[String],
    input: TypedValue,
    stdin: Option[ToolMiddlewareInputHandle],
    principal: String
  )

  trait UniversalToolMiddleware {
    def invoke(
      invocation: UniversalToolMiddlewareInvocation,
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedValue], ToolMiddlewareResult]]
  }

  final class FakeRawToolUnderlying(
    responses: List[Either[ToolInvokeError[TypedValue], ToolMiddlewareResult]]
  ) extends UniversalToolUnderlying {
    val calls: mutable.ListBuffer[RawInvocation] = mutable.ListBuffer.empty
    private val remaining                        =
      mutable.Queue.empty[Either[ToolInvokeError[TypedValue], ToolMiddlewareResult]]
    remaining ++= responses

    override def invoke(
      commandPath: List[String],
      input: TypedValue,
      stdin: Option[ToolMiddlewareInputHandle]
    ): Future[Either[ToolInvokeError[TypedValue], ToolMiddlewareResult]] = {
      calls += RawInvocation(commandPath, input, stdin)
      Future.successful(remaining.dequeue())
    }
  }

  trait PublicEchoUnderlying {
    def publicEcho(config: String): Future[Either[ToolInvokeError[Nothing], Unit]]

    def echo(
      config: String,
      value: String
    ): Future[Either[ToolInvokeError[PublicError], String]]

    def copy(
      config: String,
      stdin: ToolMiddlewareInputHandle
    ): Future[Either[ToolInvokeError[Nothing], (Long, ToolMiddlewareOutputHandle)]]

    def inspect(
      config: String,
      prefix: String,
      name: String
    ): Future[Either[ToolInvokeError[Nothing], String]]
  }

  trait BackendEchoUnderlying {
    def execute(value: String): Future[Either[ToolInvokeError[BackendError], Long]]
  }

  trait PublicEchoMiddleware extends PublicEchoMiddleware.Adapter[PublicEchoUnderlying]

  object PublicEchoMiddleware {
    trait Adapter[U] {
      def publicEcho(
        underlying: U,
        config: String
      ): Future[Either[ToolInvokeError[Nothing], Unit]]

      def echo(
        underlying: U,
        config: String,
        value: String,
        principal: String
      ): Future[Either[ToolInvokeError[PublicError], String]]

      def copy(
        underlying: U,
        config: String,
        stdin: ToolMiddlewareInputHandle
      ): Future[Either[ToolInvokeError[Nothing], (Long, ToolMiddlewareOutputHandle)]]

      def inspect(
        underlying: U,
        config: String,
        prefix: String,
        name: String
      ): Future[Either[ToolInvokeError[Nothing], String]]
    }
  }

  sealed trait PublicError
  final case class PublicRejected(message: String) extends PublicError

  sealed trait BackendError
  final case class BackendFailed(message: String) extends BackendError

  @toolMiddleware(name = "echo-policy", aliases = Array("policy"))
  final class EchoPolicy extends PublicEchoMiddleware {
    override def publicEcho(
      underlying: PublicEchoUnderlying,
      config: String
    ): Future[Either[ToolInvokeError[Nothing], Unit]] =
      underlying.publicEcho(config)

    override def echo(
      underlying: PublicEchoUnderlying,
      config: String,
      value: String,
      principal: String
    ): Future[Either[ToolInvokeError[PublicError], String]] =
      underlying.echo(config, value)

    override def copy(
      underlying: PublicEchoUnderlying,
      config: String,
      stdin: ToolMiddlewareInputHandle
    ): Future[Either[ToolInvokeError[Nothing], (Long, ToolMiddlewareOutputHandle)]] =
      underlying.copy(config, stdin)

    override def inspect(
      underlying: PublicEchoUnderlying,
      config: String,
      prefix: String,
      name: String
    ): Future[Either[ToolInvokeError[Nothing], String]] =
      underlying.inspect(config, prefix, name)
  }

  @toolMiddleware(name = "public-to-backend")
  final class PublicToBackend extends PublicEchoMiddleware.Adapter[BackendEchoUnderlying] {
    implicit private val ec: ExecutionContext = ExecutionContext.global

    override def publicEcho(
      underlying: BackendEchoUnderlying,
      config: String
    ): Future[Either[ToolInvokeError[Nothing], Unit]] =
      Future.successful(Right(()))

    override def echo(
      underlying: BackendEchoUnderlying,
      config: String,
      value: String,
      principal: String
    ): Future[Either[ToolInvokeError[PublicError], String]] =
      underlying.execute(s"$config:$value").map {
        case Right(result) => Right(result.toString)
        case Left(error)   =>
          Left(error.mapTool { case BackendFailed(message) =>
            PublicRejected(message)
          })
      }

    override def copy(
      underlying: BackendEchoUnderlying,
      config: String,
      stdin: ToolMiddlewareInputHandle
    ): Future[Either[ToolInvokeError[Nothing], (Long, ToolMiddlewareOutputHandle)]] =
      Future.successful(Left(ToolInvokeError.ConstraintViolation("copy is not supported")))

    override def inspect(
      underlying: BackendEchoUnderlying,
      config: String,
      prefix: String,
      name: String
    ): Future[Either[ToolInvokeError[Nothing], String]] =
      Future.successful(Right(s"$config:$prefix:$name"))
  }

  @universalToolMiddleware(name = "audit-all-tools")
  final class AuditAllTools extends UniversalToolMiddleware {
    override def invoke(
      invocation: UniversalToolMiddlewareInvocation,
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedValue], ToolMiddlewareResult]] =
      underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
  }
}

class ToolMiddlewareCompileContractSpec extends munit.FunSuite {
  import ToolMiddlewareCompileContract._

  test("fake raw underlying records sequential calls and returns configured responses") {
    val stdin = new ToolMiddlewareInputHandle {}
    val fake  = new FakeRawToolUnderlying(
      List(
        Right(ToolMiddlewareResult(Some(TypedValue("first")), None)),
        Left(ToolInvokeError.ConstraintViolation("retry")),
        Right(ToolMiddlewareResult(Some(TypedValue("third")), None))
      )
    )

    val middleware                                                                  = new AuditAllTools
    def invocation(value: String, stream: Option[ToolMiddlewareInputHandle] = None) =
      UniversalToolMiddlewareInvocation(
        "public-echo",
        ToolMetadata("public-echo", "1.0.0"),
        List("run"),
        TypedValue(value),
        stream,
        "principal"
      )

    val first  = Await.result(middleware.invoke(invocation("one", Some(stdin)), fake), 1.second)
    val second = Await.result(middleware.invoke(invocation("two"), fake), 1.second)
    val third  = Await.result(middleware.invoke(invocation("three"), fake), 1.second)

    assertEquals(first, Right(ToolMiddlewareResult(Some(TypedValue("first")), None)))
    assertEquals(second, Left(ToolInvokeError.ConstraintViolation("retry")))
    assertEquals(third, Right(ToolMiddlewareResult(Some(TypedValue("third")), None)))
    assertEquals(fake.calls.toList.map(_.input.value), List("one", "two", "three"))
    assertEquals(fake.calls.head.stdin, Some(stdin))
  }
}
