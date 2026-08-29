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

object ToolMiddlewareContractFixtures {

  val toolDefinitions: String =
    """|package example.middleware
       |
       |import golem.Principal
       |import golem.runtime.annotations._
       |import golem.tool.{ToolInputStream, ToolOutputStream}
       |
       |sealed trait PublicError
       |object PublicError {
       |  final case class Rejected(message: String) extends PublicError
       |}
       |
       |sealed trait BackendError
       |object BackendError {
       |  final case class Failed(message: String) extends BackendError
       |}
       |
       |@toolDefinition(name = "public-echo", version = "1.0.0")
       |trait PublicEcho {
       |  @arg("config", scope = "global")
       |  def publicEcho(config: String): Unit
       |
       |  def echo(value: String, principal: Principal): Either[PublicError, String]
       |
       |  def copy(stdin: ToolInputStream, stdout: ToolOutputStream): Long
       |
       |  def nested(prefix: String): PublicNested
       |}
       |
       |@toolDefinition(name = "public-nested", version = "1.0.0")
       |trait PublicNested {
       |  def inspect(name: String): String
       |}
       |
       |@toolDefinition(name = "backend-echo", version = "1.0.0")
       |trait BackendEcho {
       |  def execute(encoded: String): Either[BackendError, Long]
       |}
       |""".stripMargin

  val transparentMiddleware: String =
    """|package example.middleware
       |
       |import golem.runtime.annotations._
       |import golem.tool.ToolInvokeError
       |
       |import scala.concurrent.Future
       |
       |@toolMiddleware(name = "echo-policy", aliases = Array("policy"))
       |@description("Validates and forwards public echo calls")
       |final class EchoPolicy extends PublicEchoMiddleware {
       |  override def publicEcho(
       |    underlying: PublicEchoUnderlying,
       |    config: String
       |  ): Future[Either[ToolInvokeError[Nothing], Unit]] =
       |    underlying.publicEcho(config)
       |
       |  override def echo(
       |    underlying: PublicEchoUnderlying,
       |    config: String,
       |    value: String,
       |    principal: golem.Principal
       |  ): Future[Either[ToolInvokeError[PublicError], String]] =
       |    underlying.echo(config, value)
       |
       |  override def copy(
       |    underlying: PublicEchoUnderlying,
       |    config: String,
       |    stdin: golem.tool.ToolInputStream
       |  ): Future[Either[ToolInvokeError[Nothing], (Long, golem.tool.ToolOutputStream)]] =
       |    underlying.copy(config, stdin)
       |
       |  override def inspect(
       |    underlying: PublicEchoUnderlying,
       |    config: String,
       |    prefix: String,
       |    name: String
       |  ): Future[Either[ToolInvokeError[Nothing], String]] =
       |    underlying.inspect(config, prefix, name)
       |}
       |""".stripMargin

  val adapterMiddleware: String =
    """|package example.middleware
       |
       |import golem.runtime.annotations._
       |import golem.tool.ToolInvokeError
       |
       |import scala.concurrent.Future
       |import scala.concurrent.ExecutionContext.Implicits.global
       |
       |@toolMiddleware(name = "public-to-backend")
       |final class PublicToBackend extends PublicEchoMiddleware.Adapter[BackendEchoUnderlying] {
       |  override def publicEcho(
       |    underlying: BackendEchoUnderlying,
       |    config: String
       |  ): Future[Either[ToolInvokeError[Nothing], Unit]] =
       |    Future.successful(Right(()))
       |
       |  override def echo(
       |    underlying: BackendEchoUnderlying,
       |    config: String,
       |    value: String,
       |    principal: golem.Principal
       |  ): Future[Either[ToolInvokeError[PublicError], String]] =
       |    underlying.execute(s"$config:$value").map {
       |      case Right(result) => Right(result.toString)
       |      case Left(error)   => Left(error.mapTool {
       |        case BackendError.Failed(message) => PublicError.Rejected(message)
       |      })
       |    }
       |
       |  override def copy(
       |    underlying: BackendEchoUnderlying,
       |    config: String,
       |    stdin: golem.tool.ToolInputStream
       |  ): Future[Either[ToolInvokeError[Nothing], (Long, golem.tool.ToolOutputStream)]] =
       |    Future.successful(Left(ToolInvokeError.ConstraintViolation("copy is not supported")))
       |
       |  override def inspect(
       |    underlying: BackendEchoUnderlying,
       |    config: String,
       |    prefix: String,
       |    name: String
       |  ): Future[Either[ToolInvokeError[Nothing], String]] =
       |    Future.successful(Right(s"$config:$prefix:$name"))
       |}
       |""".stripMargin

  val universalMiddleware: String =
    """|package example.middleware
       |
       |import golem.runtime.annotations._
       |import golem.schema.TypedSchemaValue
       |import golem.tool._
       |
       |import scala.concurrent.Future
       |
       |@universalToolMiddleware(name = "audit-all-tools")
       |@description("Audits and forwards every tool invocation")
       |final class AuditAllTools extends UniversalToolMiddleware {
       |  override def invoke(
       |    invocation: UniversalToolMiddlewareInvocation,
       |    underlying: UniversalToolUnderlying
       |  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] =
       |    underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
       |}
       |""".stripMargin

  val invalidMiddlewareSources: Map[String, String] = Map(
    "constructor-argument" ->
      """|package example.middleware
         |@toolMiddleware(name = "invalid-constructor")
         |final class InvalidConstructor(value: String) extends PublicEchoMiddleware
         |""".stripMargin,
    "generic" ->
      """|package example.middleware
         |@toolMiddleware(name = "invalid-generic")
         |final class InvalidGeneric[A] extends PublicEchoMiddleware
         |""".stripMargin,
    "wrong-parent" ->
      """|package example.middleware
         |@toolMiddleware(name = "invalid-parent")
         |final class InvalidParent extends PublicEcho
         |""".stripMargin,
    "missing-name" ->
      """|package example.middleware
         |@toolMiddleware()
         |final class MissingName extends PublicEchoMiddleware
         |""".stripMargin,
    "unresolved-underlying" ->
      """|package example.middleware
         |@toolMiddleware(name = "invalid-underlying")
         |final class InvalidUnderlying extends PublicEchoMiddleware.Adapter[MissingUnderlying]
         |""".stripMargin,
    "flattened-collision" ->
      """|package example.middleware
         |import golem.runtime.annotations._
         |@toolDefinition(name = "collision-root")
         |trait CollisionRoot {
         |  def first(): CollisionFirst
         |  def second(): CollisionSecond
         |}
         |@toolDefinition(name = "collision-first")
         |trait CollisionFirst {
         |  def run(value: String): String
         |}
         |@toolDefinition(name = "collision-second")
         |trait CollisionSecond {
         |  def run(value: String): String
         |}
         |""".stripMargin
  )

  val ordinaryClientBaselineSource: String =
    """|package example.baseline
       |
       |import golem.Principal
       |import golem.runtime.annotations._
       |import golem.tool.{ToolInputStream, ToolOutputStream}
       |import scala.concurrent.Future
       |
       |sealed trait BaselineError
       |
       |@toolDefinition(name = "baseline-tool", version = "1.2.3")
       |trait BaselineTool {
       |  @arg("config", scope = "global")
       |  def baselineTool(config: String): Unit
       |  def run(
       |    value: String,
       |    stdin: ToolInputStream,
       |    stdout: ToolOutputStream,
       |    principal: Principal
       |  ): Future[Either[BaselineError, Long]]
       |  def nested(prefix: String): BaselineNested
       |}
       |
       |@toolDefinition(name = "baseline-nested", version = "1.2.3")
       |trait BaselineNested {
       |  def inspect(name: String): String
       |}
       |""".stripMargin

  val ordinaryClientSurface: List[String] = List(
    "trait BaselineToolClient {",
    "def baselineTool(config: String): _root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolError[_root_.scala.Nothing], _root_.scala.Unit]]",
    "def run(config: String, value: String, stdin: _root_.golem.tool.ToolInputStream): _root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolError[BaselineError], (Long, _root_.golem.tool.ToolOutputStream)]]",
    "def nested(config: String, prefix: String): BaselineToolClient.NestedClient",
    "final class NestedClient private[BaselineToolClient] (",
    "def inspect(name: String): _root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolError[_root_.scala.Nothing], String]]"
  )

  val ordinaryClientSnapshotSha256: String =
    "50920f78e984f5ad52b3b84f7b43ad2066ac34f20755112414201cc1da39274c"
}
