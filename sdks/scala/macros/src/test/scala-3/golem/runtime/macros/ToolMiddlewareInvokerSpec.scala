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
import golem.runtime.annotations.*
import golem.schema.{IntoSchema, SchemaValue, TypedSchemaValue}
import golem.tool.*
import zio.test.*

import scala.collection.mutable
import scala.concurrent.Future

object ToolMiddlewareInvokerSpec extends ZIOSpecDefault {
  import UnderlyingTestSupport.flatMapResult

  enum PublicError {
    @error(kind = "usage", exitCode = 2)
    case Rejected(message: String)
  }

  enum BackendError {
    @error(kind = "runtime", exitCode = 1)
    case Failed(message: String)
  }

  @toolDefinition(name = "middleware-echo", version = "1.0.0")
  trait MiddlewareEcho {
    @arg("config", scope = "global")
    def middlewareEcho(config: String): Unit

    def act(value: String, principal: Principal): Either[PublicError, String]

    @arg("cfg", scope = "global", aliases = Array("config"))
    def aliased(cfg: String, value: String): String

    @arg("attempts", kind = "count-flag")
    def count(attempts: Int, value: String): Int

    def pipe(stdin: ToolInputStream, stdout: ToolOutputStream): Long

    def nested(prefix: String): MiddlewareNested
  }

  @toolDefinition(name = "nested", version = "1.0.0")
  trait MiddlewareNested {
    @command(aliases = Array("look"))
    def inspect(name: String): String
  }

  @toolDefinition(name = "backend-echo", version = "2.0.0")
  trait BackendEcho {
    def execute(value: String): Either[BackendError, Long]
  }

  trait MiddlewareEchoUnderlying {
    def middlewareEcho(config: String): Future[Either[ToolInvokeError[Nothing], Unit]]
    def act(config: String, value: String): Future[Either[ToolInvokeError[PublicError], String]]
    def aliased(cfg: String, value: String): Future[Either[ToolInvokeError[Nothing], String]]
    def count(config: String, attempts: Int, value: String): Future[Either[ToolInvokeError[Nothing], Int]]
    def pipe(
      config: String,
      stdin: ToolInputStream
    ): Future[Either[ToolInvokeError[Nothing], (Long, ToolOutputStream)]]
    def inspect(
      config: String,
      prefix: String,
      name: String
    ): Future[Either[ToolInvokeError[Nothing], String]]
  }

  object MiddlewareEchoUnderlying {
    private lazy val descriptor   = ToolDefinitionMacro.tryMetadata[MiddlewareEcho]
    private lazy val publicErrors = ToolErrorSchemaDerivation.derive[PublicError]

    def fromRaw(raw: RawToolUnderlying): MiddlewareEchoUnderlying =
      new MiddlewareEchoUnderlying {
        def middlewareEcho(config: String): Future[Either[ToolInvokeError[Nothing], Unit]] =
          UnderlyingTestSupport
            .runInfallible(raw, descriptor, Nil, List("config" -> IntoSchema[String].toValue(config)), None)
            .flatMapResult(ToolUnderlyingRuntime.decodeUnitResult)

        def act(config: String, value: String): Future[Either[ToolInvokeError[PublicError], String]] =
          UnderlyingTestSupport
            .run(
              raw,
              descriptor,
              List("act"),
              List(
                "config" -> IntoSchema[String].toValue(config),
                "value"  -> IntoSchema[String].toValue(value)
              ),
              None,
              publicErrors.fromErrorPayloadValue
            )
            .flatMapResult(result => ToolUnderlyingRuntime.decodeValueResult(result, golem.schema.FromSchema[String]))

        def aliased(cfg: String, value: String): Future[Either[ToolInvokeError[Nothing], String]] =
          UnderlyingTestSupport
            .runInfallible(
              raw,
              descriptor,
              List("aliased"),
              List(
                "config" -> IntoSchema[String].toValue(cfg),
                "value"  -> IntoSchema[String].toValue(value)
              ),
              None
            )
            .flatMapResult(result => ToolUnderlyingRuntime.decodeValueResult(result, golem.schema.FromSchema[String]))

        def count(config: String, attempts: Int, value: String): Future[Either[ToolInvokeError[Nothing], Int]] =
          UnderlyingTestSupport
            .runInfallible(
              raw,
              descriptor,
              List("count"),
              List(
                "config"   -> IntoSchema[String].toValue(config),
                "attempts" -> ToolUnderlyingRuntime.countFlagValue(attempts),
                "value"    -> IntoSchema[String].toValue(value)
              ),
              None
            )
            .flatMapResult(result => ToolUnderlyingRuntime.decodeValueResult(result, golem.schema.FromSchema[Int]))

        def pipe(
          config: String,
          stdin: ToolInputStream
        ): Future[Either[ToolInvokeError[Nothing], (Long, ToolOutputStream)]] =
          UnderlyingTestSupport
            .runInfallible(
              raw,
              descriptor,
              List("pipe"),
              List("config" -> IntoSchema[String].toValue(config)),
              Some(stdin)
            )
            .flatMapResult(result =>
              ToolUnderlyingRuntime.decodeValueStdoutResult(result, golem.schema.FromSchema[Long])
            )

        def inspect(
          config: String,
          prefix: String,
          name: String
        ): Future[Either[ToolInvokeError[Nothing], String]] =
          UnderlyingTestSupport
            .runInfallible(
              raw,
              descriptor,
              List("nested", "inspect"),
              List(
                "config" -> IntoSchema[String].toValue(config),
                "prefix" -> IntoSchema[String].toValue(prefix),
                "name"   -> IntoSchema[String].toValue(name)
              ),
              None
            )
            .flatMapResult(result => ToolUnderlyingRuntime.decodeValueResult(result, golem.schema.FromSchema[String]))
      }
  }

  trait BackendEchoUnderlying {
    def execute(value: String): Future[Either[ToolInvokeError[BackendError], Long]]
  }

  object BackendEchoUnderlying {
    private lazy val descriptor    = ToolDefinitionMacro.tryMetadata[BackendEcho]
    private lazy val backendErrors = ToolErrorSchemaDerivation.derive[BackendError]

    def fromRaw(raw: RawToolUnderlying): BackendEchoUnderlying =
      new BackendEchoUnderlying {
        def execute(value: String): Future[Either[ToolInvokeError[BackendError], Long]] =
          UnderlyingTestSupport
            .run(
              raw,
              descriptor,
              List("execute"),
              List("value" -> IntoSchema[String].toValue(value)),
              None,
              backendErrors.fromErrorPayloadValue
            )
            .flatMapResult(result => ToolUnderlyingRuntime.decodeValueResult(result, golem.schema.FromSchema[Long]))
      }
  }

  trait MiddlewareEchoMiddleware extends MiddlewareEchoMiddleware.Adapter[MiddlewareEchoUnderlying]

  object MiddlewareEchoMiddleware {
    trait Adapter[U] {
      def middlewareEcho(
        underlying: U,
        @internalToolMiddlewareField("config") config: String
      ): Future[Either[ToolInvokeError[Nothing], Unit]]
      def act(
        underlying: U,
        @internalToolMiddlewareField("config") config: String,
        @internalToolMiddlewareField("value") value: String,
        principal: Principal
      ): Future[Either[ToolInvokeError[PublicError], String]]
      def aliased(
        underlying: U,
        @internalToolMiddlewareField("config") cfg: String,
        @internalToolMiddlewareField("value") value: String
      ): Future[Either[ToolInvokeError[Nothing], String]]
      def count(
        underlying: U,
        @internalToolMiddlewareField("config") config: String,
        @internalToolMiddlewareField("attempts", countFlag = true) attempts: Int,
        @internalToolMiddlewareField("value") value: String
      ): Future[Either[ToolInvokeError[Nothing], Int]]
      def pipe(
        underlying: U,
        @internalToolMiddlewareField("config") config: String,
        stdin: ToolInputStream
      ): Future[Either[ToolInvokeError[Nothing], (Long, ToolOutputStream)]]
      def inspect(
        underlying: U,
        @internalToolMiddlewareField("config") config: String,
        @internalToolMiddlewareField("prefix") prefix: String,
        @internalToolMiddlewareField("name") name: String
      ): Future[Either[ToolInvokeError[Nothing], String]]
    }
  }

  object TransparentMiddleware {
    var constructions                = 0
    var principal: Option[Principal] = None

    def reset(): Unit = {
      constructions = 0
      principal = None
    }
  }

  @toolMiddleware(name = "transparent-policy", aliases = Array("transparent"))
  @description("Transparent policy middleware")
  final class TransparentMiddleware extends MiddlewareEchoMiddleware {
    TransparentMiddleware.constructions += 1

    def middlewareEcho(
      underlying: MiddlewareEchoUnderlying,
      config: String
    ): Future[Either[ToolInvokeError[Nothing], Unit]] =
      underlying.middlewareEcho(config)

    def act(
      underlying: MiddlewareEchoUnderlying,
      config: String,
      value: String,
      principal: Principal
    ): Future[Either[ToolInvokeError[PublicError], String]] = {
      TransparentMiddleware.principal = Some(principal)
      value match {
        case "reject"      => Future.successful(Left(ToolInvokeError.Tool(PublicError.Rejected("denied"))))
        case "short"       => Future.successful(Right("short-circuit"))
        case "throw"       => throw new IllegalStateException("middleware-threw")
        case "fail-future" => Future.failed(new IllegalStateException("middleware-future-failed"))
        case "retry"       =>
          underlying
            .act(config, value)
            .flatMap {
              case Left(_) => underlying.act(config, value)
              case success => Future.successful(success)
            }(ToolInvokerRuntime.executionContext)
        case _ =>
          underlying.act(config, value).map(_.map(_.toUpperCase))(ToolInvokerRuntime.executionContext)
      }
    }

    def pipe(
      underlying: MiddlewareEchoUnderlying,
      config: String,
      stdin: ToolInputStream
    ): Future[Either[ToolInvokeError[Nothing], (Long, ToolOutputStream)]] =
      underlying.pipe(config, stdin)

    def aliased(
      underlying: MiddlewareEchoUnderlying,
      cfg: String,
      value: String
    ): Future[Either[ToolInvokeError[Nothing], String]] =
      underlying.aliased(cfg, value)

    def count(
      underlying: MiddlewareEchoUnderlying,
      config: String,
      attempts: Int,
      value: String
    ): Future[Either[ToolInvokeError[Nothing], Int]] =
      underlying.count(config, attempts, value)

    def inspect(
      underlying: MiddlewareEchoUnderlying,
      config: String,
      prefix: String,
      name: String
    ): Future[Either[ToolInvokeError[Nothing], String]] =
      underlying.inspect(config, prefix, name)
  }

  @toolMiddleware(name = "adapter-policy")
  final class AdapterMiddleware extends MiddlewareEchoMiddleware.Adapter[BackendEchoUnderlying] {
    def middlewareEcho(
      underlying: BackendEchoUnderlying,
      config: String
    ): Future[Either[ToolInvokeError[Nothing], Unit]] =
      Future.successful(Right(()))

    def act(
      underlying: BackendEchoUnderlying,
      config: String,
      value: String,
      principal: Principal
    ): Future[Either[ToolInvokeError[PublicError], String]] =
      underlying
        .execute(s"$config:$value")
        .map {
          case Right(result) => Right(s"adapted-$result")
          case Left(error)   =>
            Left(error.mapTool { case BackendError.Failed(message) =>
              PublicError.Rejected(s"adapted-$message")
            })
        }(ToolInvokerRuntime.executionContext)

    def pipe(
      underlying: BackendEchoUnderlying,
      config: String,
      stdin: ToolInputStream
    ): Future[Either[ToolInvokeError[Nothing], (Long, ToolOutputStream)]] =
      Future.successful(Left(ToolInvokeError.ConstraintViolation("adapter has no pipe")))

    def aliased(
      underlying: BackendEchoUnderlying,
      cfg: String,
      value: String
    ): Future[Either[ToolInvokeError[Nothing], String]] =
      Future.successful(Right(s"$cfg:$value"))

    def count(
      underlying: BackendEchoUnderlying,
      config: String,
      attempts: Int,
      value: String
    ): Future[Either[ToolInvokeError[Nothing], Int]] =
      Future.successful(Right(attempts))

    def inspect(
      underlying: BackendEchoUnderlying,
      config: String,
      prefix: String,
      name: String
    ): Future[Either[ToolInvokeError[Nothing], String]] =
      Future.successful(Right(s"$config:$prefix:$name"))
  }

  object UniversalMiddleware {
    var constructions: Int                                  = 0
    var observed: Option[UniversalToolMiddlewareInvocation] = None
    var observedUnderlying: Option[UniversalToolUnderlying] = None
    var configuredFinal: Option[ToolInvokeResult]           = None

    def reset(): Unit = {
      constructions = 0
      observed = None
      observedUnderlying = None
      configuredFinal = None
    }
  }

  @universalToolMiddleware(name = "universal-policy", aliases = Array("universal"))
  @description("Universal policy middleware")
  final class UniversalMiddleware extends UniversalToolMiddleware {
    UniversalMiddleware.constructions += 1

    def invoke(
      invocation: UniversalToolMiddlewareInvocation,
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] = {
      UniversalMiddleware.observed = Some(invocation)
      UniversalMiddleware.observedUnderlying = Some(underlying)
      invocation.commandPath match {
        case List("reject")           => Future.successful(Left(ToolInvokeError.Tool(invocation.input)))
        case List("short")            => Future.successful(Right(ToolInvokeResult(Some(invocation.input), None)))
        case List("configured-final") => Future.successful(Right(UniversalMiddleware.configuredFinal.get))
        case List("throw")            => throw new IllegalStateException("universal-middleware-threw")
        case List("fail-future")      =>
          Future.failed(new IllegalStateException("universal-middleware-future-failed"))
        case List("retry") =>
          underlying
            .invoke(invocation.commandPath, invocation.input, invocation.stdin)
            .flatMap {
              case Left(_) => underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
              case success => Future.successful(success)
            }(ToolInvokerRuntime.executionContext)
        case List("transform") =>
          underlying
            .invoke(invocation.commandPath, invocation.input, invocation.stdin)
            .map(_.map(_.copy(result = Some(IntoSchema[String].toTyped("transformed")))))(using
              ToolInvokerRuntime.executionContext
            )
        case _ => underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
      }
    }
  }

  private object UnderlyingTestSupport {
    extension [E](call: Future[Either[ToolInvokeError[E], ToolInvokeResult]])
      def flatMapResult[A](
        decode: ToolInvokeResult => Either[ToolError[Nothing], A]
      ): Future[Either[ToolInvokeError[E], A]] =
        ToolUnderlyingRuntime.complete(call)(decode)

    def runInfallible(
      raw: RawToolUnderlying,
      descriptor: Either[ToolBuildError, ExtendedToolType],
      path: List[String],
      params: List[(String, SchemaValue)],
      stdin: Option[ToolInputStream]
    ): Future[Either[ToolInvokeError[Nothing], ToolInvokeResult]] =
      ToolUnderlyingRuntime.runInfallible(raw, descriptor, path, encode(descriptor, path, params), stdin)

    def run[E](
      raw: RawToolUnderlying,
      descriptor: Either[ToolBuildError, ExtendedToolType],
      path: List[String],
      params: List[(String, SchemaValue)],
      stdin: Option[ToolInputStream],
      decodeError: TypedSchemaValue => Either[String, E]
    ): Future[Either[ToolInvokeError[E], ToolInvokeResult]] =
      ToolUnderlyingRuntime.run(raw, descriptor, path, encode(descriptor, path, params), stdin, decodeError)

    private def encode(
      descriptor: Either[ToolBuildError, ExtendedToolType],
      path: List[String],
      params: List[(String, SchemaValue)]
    ): Either[ToolInvokeError[Nothing], TypedSchemaValue] =
      ToolUnderlyingRuntime.buildInputFromModel(
        ToolUnderlyingRuntime.staticInputModel(descriptor, path),
        params
      )
  }

  private final class FakeStdin(val id: String) extends ToolInputStream {
    var closeCount = 0

    override private[golem] def close(): Future[Unit] = {
      closeCount += 1
      Future.successful(())
    }
  }
  private object FakeStdin {
    def apply(id: String): FakeStdin = new FakeStdin(id)
  }

  private final class FakeStdout(val id: String) extends ToolOutputStream {
    var closeCount = 0

    override private[golem] def close(): Future[Unit] = {
      closeCount += 1
      Future.successful(())
    }
  }
  private object FakeStdout {
    def apply(id: String): FakeStdout = new FakeStdout(id)
  }

  private final case class RawCall(
    path: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream]
  )

  private final class FakeRaw(responses: List[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]])
      extends RawToolUnderlying {
    val calls: mutable.ListBuffer[RawCall] = mutable.ListBuffer.empty
    private val remaining                  = mutable.Queue.from(responses)

    def invoke(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolInputStream]
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] = {
      calls += RawCall(commandPath, input, stdin)
      Future.successful(remaining.dequeue())
    }
  }

  private val anonymous = Principal.Anonymous

  private lazy val transparentHandle =
    ToolMiddlewareMacro.transparentHandle[
      MiddlewareEcho,
      MiddlewareEchoUnderlying,
      MiddlewareEchoMiddleware,
      TransparentMiddleware
    ](MiddlewareEchoUnderlying.fromRaw)

  private lazy val adapterHandle =
    ToolMiddlewareMacro.adapterHandle[
      MiddlewareEcho,
      BackendEcho,
      BackendEchoUnderlying,
      MiddlewareEchoMiddleware.Adapter[BackendEchoUnderlying],
      AdapterMiddleware
    ](BackendEchoUnderlying.fromRaw)

  private lazy val universalHandle =
    ToolMiddlewareMacro.universalHandle[UniversalMiddleware]

  private def built[A](result: Either[ToolBuildError, A]): A =
    result.fold(error => throw new IllegalStateException(error.message), identity)

  private lazy val presented = built(transparentHandle.presented(new ToolBuildCtx))

  private def outcome[A](future: Future[A]): A =
    future.value.getOrElse(throw new IllegalStateException("future did not complete synchronously")).get

  private def invocationInput(path: List[String], values: SchemaValue*): TypedSchemaValue = {
    val index  = presented.commandIndexByPath(path).get
    val schema = presented.canonicalInputRecordSchema(index).toOption.get
    TypedSchemaValue(schema, SchemaValue.RecordValue(values.toList))
  }

  private def invoke(
    handle: MonomorphicToolMiddlewareHandle,
    raw: RawToolUnderlying,
    path: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream] = None,
    principal: Principal = anonymous
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] =
    ToolMiddlewareInvokerRuntime.invoke(
      presented,
      handle,
      raw,
      presented.toolName,
      path,
      input,
      stdin,
      principal
    )

  private def invokeUniversal(
    raw: RawToolUnderlying,
    path: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream] = None,
    principal: Principal = anonymous,
    toolName: String = presented.toolName
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult]] =
    UniversalToolMiddlewareInvokerRuntime.invoke(
      universalHandle,
      raw,
      toolName,
      presented.tryToTool.toOption.get,
      path,
      input,
      stdin,
      principal
    )

  private def success[A: IntoSchema](value: A): Either[ToolInvokeError[TypedSchemaValue], ToolInvokeResult] =
    Right(ToolInvokeResult(Some(IntoSchema[A].toTyped(value)), None))

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolMiddlewareInvokerSpec")(
      test("descriptor metadata and transparent expected descriptor are derived") {
        val descriptor = built(transparentHandle.descriptor(new ToolBuildCtx))
        val expected   = built(transparentHandle.expected(new ToolBuildCtx))
        assertTrue(
          descriptor.name == "transparent-policy",
          descriptor.aliases == List("transparent"),
          descriptor.doc.description == "Transparent policy middleware",
          descriptor.scope == ToolMiddlewareScope.Monomorphic(
            presented.tryToTool.toOption.get,
            Some(expected.tryToTool.toOption.get)
          ),
          expected.toolName == presented.toolName
        )
      },
      test("short-circuit and typed rejection do not call the underlying") {
        val raw   = new FakeRaw(Nil)
        val short = outcome(
          invoke(
            transparentHandle,
            raw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("short")
            )
          )
        )
        val rejected = outcome(
          invoke(
            transparentHandle,
            raw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("reject")
            )
          )
        )
        assertTrue(
          short == success("short-circuit"),
          rejected == Left(ToolInvokeError.Tool(IntoSchema[String].toTyped("denied"))),
          raw.calls.isEmpty
        )
      },
      test("forwarding injects principal and transforms the result") {
        TransparentMiddleware.reset()
        val raw       = new FakeRaw(List(success("from-underlying")))
        val principal = Principal.Oidc("alice", "issuer", "{}")
        val result    = outcome(
          invoke(
            transparentHandle,
            raw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("forward")
            ),
            principal = principal
          )
        )
        assertTrue(
          result == success("FROM-UNDERLYING"),
          TransparentMiddleware.principal.contains(principal),
          raw.calls.map(_.path).toList == List(List("act"))
        )
      },
      test("canonical count flags and alias-based global redeclarations decode through the projection") {
        val aliasRaw = new FakeRaw(List(success("aliased")))
        val aliased  = outcome(
          invoke(
            transparentHandle,
            aliasRaw,
            List("aliased"),
            invocationInput(
              List("aliased"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("value")
            )
          )
        )
        val countRaw = new FakeRaw(List(success(3)))
        val counted  = outcome(
          invoke(
            transparentHandle,
            countRaw,
            List("count"),
            invocationInput(
              List("count"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("value"),
              SchemaValue.U32Value(3L)
            )
          )
        )
        assertTrue(
          aliased == success("aliased"),
          counted == success(3),
          aliasRaw.calls.head.path == List("aliased"),
          countRaw.calls.head.path == List("count")
        )
      },
      test("sequential retry reuses the invocation-scoped underlying") {
        val raw = new FakeRaw(
          List(
            Left(ToolInvokeError.ConstraintViolation("retry")),
            success("second")
          )
        )
        val result = outcome(
          invoke(
            transparentHandle,
            raw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("retry")
            )
          )
        )
        assertTrue(result == success("second"), raw.calls.size == 2)
      },
      test("all protocol errors are preserved through typed underlying and final dispatch") {
        val errors: List[ToolInvokeError[TypedSchemaValue]] = List(
          ToolInvokeError.InvalidToolName("missing"),
          ToolInvokeError.InvalidCommandPath(List("bad")),
          ToolInvokeError.InvalidInput("input"),
          ToolInvokeError.ConstraintViolation("constraint"),
          ToolInvokeError.InvalidResult("result")
        )
        val results = errors.map { error =>
          val raw = new FakeRaw(List(Left(error)))
          outcome(
            invoke(
              transparentHandle,
              raw,
              List("act"),
              invocationInput(
                List("act"),
                SchemaValue.StringValue("cfg"),
                SchemaValue.StringValue("forward")
              )
            )
          )
        }
        assertTrue(results == errors.map(Left(_)))
      },
      test("adapter uses a distinct expected descriptor and converts values and custom errors") {
        val expected     = built(adapterHandle.expected(new ToolBuildCtx))
        val descriptor   = built(adapterHandle.descriptor(new ToolBuildCtx))
        val backendError = ToolErrorSchemaDerivation
          .derive[BackendError]
          .toErrorPayloadValue(BackendError.Failed("backend"))
          .toOption
          .get
        val successRaw    = new FakeRaw(List(success(42L)))
        val successResult = outcome(
          invoke(
            adapterHandle,
            successRaw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("value")
            )
          )
        )
        val errorRaw    = new FakeRaw(List(Left(ToolInvokeError.Tool(backendError))))
        val errorResult = outcome(
          invoke(
            adapterHandle,
            errorRaw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("value")
            )
          )
        )
        assertTrue(
          presented.toolName == "middleware-echo",
          expected.toolName == "backend-echo",
          descriptor.scope == ToolMiddlewareScope.Monomorphic(
            presented.tryToTool.toOption.get,
            Some(expected.tryToTool.toOption.get)
          ),
          successRaw.calls.map(_.path).toList == List(List("execute")),
          successResult == success("adapted-42"),
          errorResult == Left(ToolInvokeError.Tool(IntoSchema[String].toTyped("adapted-backend")))
        )
      },
      test("nested command aliases dispatch to the flattened middleware leaf") {
        val raw    = new FakeRaw(List(success("nested")))
        val result = outcome(
          invoke(
            transparentHandle,
            raw,
            List("nested", "look"),
            invocationInput(
              List("nested", "look"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("prefix"),
              SchemaValue.StringValue("name")
            )
          )
        )
        assertTrue(result == success("nested"), raw.calls.head.path == List("nested", "inspect"))
      },
      test("stdin and stdout are projected through the typed facade") {
        val stdin  = FakeStdin("in")
        val stdout = FakeStdout("out")
        val raw    = new FakeRaw(List(Right(ToolInvokeResult(Some(IntoSchema[Long].toTyped(7L)), Some(stdout)))))
        val result = outcome(
          invoke(
            transparentHandle,
            raw,
            List("pipe"),
            invocationInput(List("pipe"), SchemaValue.StringValue("cfg")),
            Some(stdin)
          )
        )
        assertTrue(
          result.toOption.exists(value =>
            value.result.contains(IntoSchema[Long].toTyped(7L)) && value.stdout.contains(stdout)
          ),
          raw.calls.head.stdin.contains(stdin),
          stdin.closeCount == 0,
          stdout.closeCount == 0
        )
      },
      test("dispatch rejects malformed input and stdin shape before calling user code") {
        val raw       = new FakeRaw(Nil)
        val malformed = TypedSchemaValue(
          presented.canonicalInputRecordSchema(presented.commandIndexByPath(List("act")).get).toOption.get,
          SchemaValue.RecordValue(Nil)
        )
        val malformedResult    = outcome(invoke(transparentHandle, raw, List("act"), malformed))
        val incompatibleSchema = outcome(
          invoke(
            transparentHandle,
            raw,
            List("act"),
            IntoSchema[String].toTyped("not a canonical record")
          )
        )
        val unexpectedInput = FakeStdin("unexpected")
        val unexpectedStdin = outcome(
          invoke(
            transparentHandle,
            raw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("short")
            ),
            Some(unexpectedInput)
          )
        )
        val missingStdin = outcome(
          invoke(
            transparentHandle,
            raw,
            List("pipe"),
            invocationInput(List("pipe"), SchemaValue.StringValue("cfg"))
          )
        )
        assertTrue(
          malformedResult.isLeft,
          incompatibleSchema == Left(
            ToolInvokeError.InvalidInput("tool invocation input schema does not match the presented command")
          ),
          unexpectedStdin == Left(
            ToolInvokeError.InvalidInput("tool invocation contained unexpected stdin stream")
          ),
          missingStdin == Left(
            ToolInvokeError.InvalidInput("tool invocation did not contain declared stdin stream")
          ),
          unexpectedInput.closeCount == 1,
          raw.calls.isEmpty
        )
      },
      test("malformed underlying result and stdout shapes become InvalidResult") {
        val wrongValueRaw = new FakeRaw(List(success(7L)))
        val wrongValue    = outcome(
          invoke(
            transparentHandle,
            wrongValueRaw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("forward")
            )
          )
        )
        val unexpectedStream    = FakeStdout("extra")
        val unexpectedStdoutRaw = new FakeRaw(
          List(Right(ToolInvokeResult(Some(IntoSchema[String].toTyped("value")), Some(unexpectedStream))))
        )
        val unexpectedStdout = outcome(
          invoke(
            transparentHandle,
            unexpectedStdoutRaw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("forward")
            )
          )
        )
        val missingStdoutRaw = new FakeRaw(
          List(Right(ToolInvokeResult(Some(IntoSchema[Long].toTyped(7L)), None)))
        )
        val missingStdout = outcome(
          invoke(
            transparentHandle,
            missingStdoutRaw,
            List("pipe"),
            invocationInput(List("pipe"), SchemaValue.StringValue("cfg")),
            Some(FakeStdin("in"))
          )
        )
        assertTrue(
          wrongValue.isLeft,
          unexpectedStdout == Left(
            ToolInvokeError.InvalidResult("tool result unexpectedly contained stdout stream")
          ),
          missingStdout == Left(
            ToolInvokeError.InvalidResult("tool result did not contain declared stdout stream")
          ),
          unexpectedStream.closeCount == 1
        )
      },
      test("expected and presented result schema carriers are validated before and after typed dispatch") {
        val malformedExpectedRaw = new FakeRaw(
          List(
            Right(
              ToolInvokeResult(
                Some(TypedSchemaValue(IntoSchema[Long].graph, SchemaValue.StringValue("decodable"))),
                None
              )
            )
          )
        )
        val malformedExpected = outcome(
          invoke(
            transparentHandle,
            malformedExpectedRaw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("forward")
            )
          )
        )
        val malformedErrorRaw = new FakeRaw(
          List(
            Left(
              ToolInvokeError.Tool(
                TypedSchemaValue(IntoSchema[Long].graph, SchemaValue.StringValue("backend"))
              )
            )
          )
        )
        val malformedError = outcome(
          invoke(
            adapterHandle,
            malformedErrorRaw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("forward")
            )
          )
        )
        val malformedPresentedHandle = transparentHandle.copy(
          bindings = transparentHandle.bindings.map { binding =>
            if (binding.commandPath == List("act"))
              binding.copy(run =
                (_, _, _) =>
                  Future.successful(
                    Right(
                      ToolInvokeResult(
                        Some(TypedSchemaValue(IntoSchema[String].graph, SchemaValue.S32Value(1))),
                        None
                      )
                    )
                  )
              )
            else binding
          }
        )
        val malformedPresented = outcome(
          invoke(
            malformedPresentedHandle,
            new FakeRaw(Nil),
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("short")
            )
          )
        )
        assertTrue(
          malformedExpected.isLeft,
          malformedError.left.exists(_.isInstanceOf[ToolInvokeError.InvalidResult]),
          malformedPresented.isLeft
        )
      },
      test("unknown presented tool and command names are rejected") {
        val raw   = new FakeRaw(Nil)
        val input = invocationInput(
          List("act"),
          SchemaValue.StringValue("cfg"),
          SchemaValue.StringValue("short")
        )
        val wrongTool = outcome(
          ToolMiddlewareInvokerRuntime.invoke(
            presented,
            transparentHandle,
            raw,
            "other",
            List("act"),
            input,
            None,
            anonymous
          )
        )
        val wrongCommand = outcome(invoke(transparentHandle, raw, List("missing"), input))
        assertTrue(
          wrongTool == Left(ToolInvokeError.InvalidToolName("other")),
          wrongCommand == Left(ToolInvokeError.InvalidCommandPath(List("missing"))),
          raw.calls.isEmpty
        )
      },
      test("a fresh implementation is constructed once per invocation") {
        TransparentMiddleware.reset()
        val raw   = new FakeRaw(Nil)
        val first = outcome(
          invoke(
            transparentHandle,
            raw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("short")
            )
          )
        )
        val second = outcome(
          invoke(
            transparentHandle,
            raw,
            List("act"),
            invocationInput(
              List("act"),
              SchemaValue.StringValue("cfg"),
              SchemaValue.StringValue("short")
            )
          )
        )
        assertTrue(first.isRight, second.isRight, TransparentMiddleware.constructions == 2)
      },
      test("thrown exceptions and failed futures remain failures") {
        val raw    = new FakeRaw(Nil)
        val thrown = invoke(
          transparentHandle,
          raw,
          List("act"),
          invocationInput(
            List("act"),
            SchemaValue.StringValue("cfg"),
            SchemaValue.StringValue("throw")
          )
        )
        val failed = invoke(
          transparentHandle,
          raw,
          List("act"),
          invocationInput(
            List("act"),
            SchemaValue.StringValue("cfg"),
            SchemaValue.StringValue("fail-future")
          )
        )
        assertTrue(
          thrown.value.exists(_.failed.toOption.exists(_.getMessage == "middleware-threw")),
          failed.value.exists(_.failed.toOption.exists(_.getMessage == "middleware-future-failed"))
        )
      },
      test("universal descriptor and raw invocation context are preserved") {
        UniversalMiddleware.reset()
        val input     = IntoSchema[String].toTyped("input")
        val stdin     = FakeStdin("universal-in")
        val stdout    = FakeStdout("universal-out")
        val principal = Principal.Oidc("bob", "issuer", "{}")
        val raw       = new FakeRaw(List(Right(ToolInvokeResult(Some(input), Some(stdout)))))
        val result    = outcome(
          invokeUniversal(
            raw,
            List("nested", "forward"),
            input,
            Some(stdin),
            principal,
            "runtime-tool"
          )
        )
        val observed = UniversalMiddleware.observed.get
        assertTrue(
          universalHandle.descriptor.name == "universal-policy",
          universalHandle.descriptor.aliases == List("universal"),
          universalHandle.descriptor.doc.description == "Universal policy middleware",
          universalHandle.descriptor.scope == ToolMiddlewareScope.Universal,
          observed.toolName == "runtime-tool",
          observed.toolMetadata == presented.tryToTool.toOption.get,
          observed.commandPath == List("nested", "forward"),
          observed.input == input,
          observed.stdin.contains(stdin),
          observed.principal == principal,
          UniversalMiddleware.observedUnderlying.nonEmpty,
          raw.calls.toList == List(RawCall(List("nested", "forward"), input, Some(stdin))),
          result.toOption.exists(value => value.result.contains(input) && value.stdout.contains(stdout)),
          stdin.closeCount == 0,
          stdout.closeCount == 0
        )
      },
      test("universal middleware rejects, short-circuits, retries, and transforms raw results") {
        val input = IntoSchema[String].toTyped("input")

        val rejectRaw = new FakeRaw(Nil)
        val rejected  = outcome(invokeUniversal(rejectRaw, List("reject"), input))
        val shortRaw  = new FakeRaw(Nil)
        val short     = outcome(invokeUniversal(shortRaw, List("short"), input))

        val retryRaw = new FakeRaw(
          List(
            Left(ToolInvokeError.ConstraintViolation("retry")),
            Right(ToolInvokeResult(Some(input), None))
          )
        )
        val retried = outcome(invokeUniversal(retryRaw, List("retry"), input))

        val transformRaw = new FakeRaw(List(Right(ToolInvokeResult(Some(input), None))))
        val transformed  = outcome(invokeUniversal(transformRaw, List("transform"), input))

        assertTrue(
          rejected == Left(ToolInvokeError.Tool(input)),
          short == Right(ToolInvokeResult(Some(input), None)),
          rejectRaw.calls.isEmpty,
          shortRaw.calls.isEmpty,
          retried == Right(ToolInvokeResult(Some(input), None)),
          retryRaw.calls.size == 2,
          transformed == success("transformed"),
          transformRaw.calls.size == 1
        )
      },
      test("universal middleware preserves protocol errors and raw custom payloads") {
        val input                                           = IntoSchema[String].toTyped("input")
        val errors: List[ToolInvokeError[TypedSchemaValue]] = List(
          ToolInvokeError.InvalidToolName("missing"),
          ToolInvokeError.InvalidCommandPath(List("bad")),
          ToolInvokeError.InvalidInput("input"),
          ToolInvokeError.ConstraintViolation("constraint"),
          ToolInvokeError.InvalidResult("result"),
          ToolInvokeError.Tool(input)
        )
        val results = errors.map { error =>
          outcome(invokeUniversal(new FakeRaw(List(Left(error))), List("forward"), input))
        }
        assertTrue(results == errors.map(Left(_)))
      },
      test("universal dispatch validates raw input and final success and error carriers") {
        val input              = IntoSchema[String].toTyped("input")
        val malformed          = TypedSchemaValue(IntoSchema[String].graph, SchemaValue.S32Value(1))
        val invalidInputStream = FakeStdin("invalid-input")
        val invalidRawOutput   = FakeStdout("invalid-raw")
        val invalidFinalOutput = FakeStdout("invalid-final")

        UniversalMiddleware.reset()
        val invalidInputRaw = new FakeRaw(Nil)
        val invalidInput    = outcome(
          invokeUniversal(invalidInputRaw, List("forward"), malformed, Some(invalidInputStream))
        )
        val observedInvalidInput = UniversalMiddleware.observed
        val invalidSuccess       = outcome(
          invokeUniversal(
            new FakeRaw(List(Right(ToolInvokeResult(Some(malformed), Some(invalidRawOutput))))),
            List("forward"),
            input
          )
        )
        val invalidError = outcome(
          invokeUniversal(
            new FakeRaw(List(Left(ToolInvokeError.Tool(malformed)))),
            List("forward"),
            input
          )
        )
        UniversalMiddleware.configuredFinal = Some(ToolInvokeResult(Some(malformed), Some(invalidFinalOutput)))
        val invalidFinal = outcome(
          invokeUniversal(new FakeRaw(Nil), List("configured-final"), input)
        )
        assertTrue(
          invalidInput.isLeft,
          observedInvalidInput.isEmpty,
          invalidInputRaw.calls.isEmpty,
          invalidInputStream.closeCount == 1,
          UniversalMiddleware.constructions == 4,
          invalidSuccess.isLeft,
          invalidError.isLeft,
          invalidFinal.isLeft,
          invalidRawOutput.closeCount == 1,
          invalidFinalOutput.closeCount == 1,
          List(invalidSuccess, invalidError, invalidFinal).forall {
            case Left(_: ToolInvokeError.InvalidResult) => true
            case _                                      => false
          }
        )
      },
      test("universal implementations are invocation-fresh and failures remain failures") {
        UniversalMiddleware.reset()
        val input  = IntoSchema[String].toTyped("input")
        val first  = outcome(invokeUniversal(new FakeRaw(Nil), List("short"), input))
        val second = outcome(invokeUniversal(new FakeRaw(Nil), List("short"), input))
        val thrown = invokeUniversal(new FakeRaw(Nil), List("throw"), input)
        val failed = invokeUniversal(new FakeRaw(Nil), List("fail-future"), input)
        assertTrue(
          first.isRight,
          second.isRight,
          UniversalMiddleware.constructions == 4,
          thrown.value.exists(_.failed.toOption.exists(_.getMessage == "universal-middleware-threw")),
          failed.value.exists(_.failed.toOption.exists(_.getMessage == "universal-middleware-future-failed"))
        )
      }
    ) @@ TestAspect.sequential
}
