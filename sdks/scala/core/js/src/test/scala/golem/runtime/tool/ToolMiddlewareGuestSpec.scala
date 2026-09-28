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

package golem.runtime.tool

import golem.host.js.schema.JsTypedSchemaValue
import golem.host.js.tool._
import golem.host.{SchemaWireInterop, ToolWireInterop}
import golem.runtime.guest.ToolMiddlewareGuest
import golem.runtime.tool.host.ToolHostApi
import golem.schema.{FromSchema, IntoSchema, SchemaValue, TypedSchemaValue}
import golem.schema.wire.{SchemaWire, WitTypedSchemaValue}
import golem.tool._
import golem.tool.wire.{WitCustomToolError, WitToolError}
import golem.{FutureInterop, Principal}
import zio.test._
import zio.ZIO
import zio.blocks.schema.Schema

import scala.concurrent.Future
import scala.scalajs.js

object ToolMiddlewareGuestSpec extends ZIOSpecDefault {
  import ToolTestFixtures._

  private val universalName      = "guest-middleware-universal"
  private val typedUniversalName = "guest-middleware-typed-universal"
  private val monomorphicName    = "guest-middleware-monomorphic"
  private val typedStdoutName    = "guest-middleware-typed-stdout"
  private val universalTool      = richTool("guest-middleware-tool")
  private val monomorphicTool    = echoTool("guest-middleware-echo")
  private val typedStdoutTool    = {
    val tool = echoTool("guest-middleware-stdout")
    tool.copy(commands =
      tool.commands.map(command =>
        command.copy(body =
          command.body.map(body =>
            body.copy(stdout = Some(StreamSpec(Doc.empty, List("application/octet-stream"), required = true)))
          )
        )
      )
    )
  }
  private val anonymous       = js.Dynamic.literal("tag" -> "anonymous")
  private val noStdin: js.Any = js.undefined.asInstanceOf[js.Any]

  private final case class NestedParameters(label: String, nested: NestedParameter) derives Schema
  private final case class NestedParameter(values: List[Int]) derives Schema

  private def guest: js.Dynamic = ToolMiddlewareGuest.golemTool010ToolMiddlewareGuest

  private def typed(value: String): WitTypedSchemaValue =
    SchemaWire.typedSchemaValueToWit(TypedSchemaValue(strGraph, SchemaValue.StringValue(value)))

  private def input(value: String): JsTypedSchemaValue =
    SchemaWireInterop.typedToJs(typed(value))

  private val noParameters: JsTypedSchemaValue =
    SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(ToolMiddleware.noParametersValue))

  private lazy val monomorphicInput: JsTypedSchemaValue = {
    val schema = monomorphicTool.canonicalInputRecordSchema(0).toOption.get
    SchemaWireInterop.typedToJs(
      SchemaWire.typedSchemaValueToWit(
        TypedSchemaValue(schema, SchemaValue.RecordValue(List(SchemaValue.StringValue("hello"))))
      )
    )
  }

  private def toolToJs(tool: ExtendedToolType): JsTool =
    ToolWireInterop.toolToJs(tool.tryToTool.toOption.get)

  private def fromPromise[A](promise: js.Promise[A]): ZIO[Any, Throwable, A] =
    ZIO.fromFuture(_ => FutureInterop.fromPromise(promise))

  private def rejectionOf[A](promise: js.Promise[A]): ZIO[Any, Nothing, Any] =
    fromPromise(promise).flip.orDieWith(_ => new RuntimeException("expected promise rejection")).map {
      case js.JavaScriptException(value) => value
      case other                         => throw other
    }

  private def resolved(value: JsInvocationResult): js.Promise[JsInvocationResult] =
    FutureInterop.toPromise(Future.successful(value))

  private def resultOf(value: JsInvocationResult): Option[JsTypedSchemaValue] = {
    val result = value.asInstanceOf[js.Dynamic].selectDynamic("result")
    if (js.isUndefined(result)) None else Some(result.asInstanceOf[JsTypedSchemaValue])
  }

  private def stdoutOf(value: JsInvocationResult): JsWasiOutputStream =
    value.asInstanceOf[js.Dynamic].selectDynamic("stdout").asInstanceOf[JsWasiOutputStream]

  private def wrapped(
    invoke: (js.Array[String], JsTypedSchemaValue, js.Any) => js.Promise[JsInvocationResult]
  ): JsUnderlyingTool =
    js.Dynamic
      .literal(
        "invoke" -> js.Any.fromFunction3((path: js.Array[String], input: JsTypedSchemaValue, stdin: js.Any) =>
          FutureInterop.toPromise(
            FutureInterop
              .fromPromise(invoke(path, input, stdin))
              .map(result =>
                js.Tuple2(
                  js.Dynamic
                    .literal(
                      "get"    -> js.Any.fromFunction0(() => js.Promise.resolve(result.result)),
                      "cancel" -> js.Any.fromFunction0(() => ())
                    )
                    .asInstanceOf[JsUnderlyingInvokeResult],
                  result.stdout.asInstanceOf[js.UndefOr[JsWasiOutputStream]]
                )
              )(ToolInvokerRuntime.executionContext)
          )
        )
      )
      .asInstanceOf[JsUnderlyingTool]

  private def invoke(
    middlewareName: String,
    toolName: String,
    metadata: JsTool,
    commandPath: js.Array[String],
    invocationInput: JsTypedSchemaValue,
    stdin: js.Any,
    underlying: JsUnderlyingTool,
    principal: js.Dynamic = anonymous,
    stdout: js.UndefOr[ToolHostApi.RawToolStdoutWriter] = js.undefined
  ): js.Promise[JsInvocationResult] =
    invokeWithParameters(
      middlewareName,
      toolName,
      metadata,
      noParameters,
      commandPath,
      invocationInput,
      stdin,
      underlying,
      principal,
      stdout
    )

  private def invokeWithParameters(
    middlewareName: String,
    toolName: String,
    metadata: JsTool,
    parameters: JsTypedSchemaValue,
    commandPath: js.Array[String],
    invocationInput: JsTypedSchemaValue,
    stdin: js.Any,
    underlying: JsUnderlyingTool,
    principal: js.Dynamic = anonymous,
    stdout: js.UndefOr[ToolHostApi.RawToolStdoutWriter] = js.undefined
  ): js.Promise[JsInvocationResult] =
    guest
      .invokeToolMiddleware(
        middlewareName,
        toolName,
        metadata,
        parameters,
        commandPath,
        invocationInput,
        stdin,
        stdout,
        principal,
        underlying
      )
      .asInstanceOf[js.Promise[JsInvocationResult]]

  private final class ForwardingUniversal extends UniversalToolMiddleware {
    def invoke(
      invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] = {
      UniversalCaptured.toolName = invocation.toolName
      UniversalCaptured.toolMetadata = Some(invocation.toolMetadata)
      UniversalCaptured.principal = invocation.principal
      underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
    }
  }

  private object UniversalCaptured {
    var toolName: String                              = ""
    var toolMetadata: Option[golem.tool.wire.WitTool] = None
    var principal: Principal                          = Principal.Anonymous
  }

  private object TypedUniversalCaptured {
    var parameters: List[NestedParameters] = Nil
  }

  private final class TypedUniversal extends UniversalToolMiddleware.WithParameters[NestedParameters] {
    def invoke(
      invocation: UniversalToolMiddlewareInvocation[NestedParameters],
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] = {
      TypedUniversalCaptured.parameters = TypedUniversalCaptured.parameters :+ invocation.parameters
      underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
    }
  }

  private object MonomorphicCaptured {
    var principal: Principal  = Principal.Anonymous
    var stdinPresent: Boolean = false
  }

  private final class CountingNonJsOutput extends ToolMiddlewareOutputHandle {
    var closeCount = 0

    override private[golem] def close(): Future[Unit] = {
      closeCount += 1
      Future.successful(())
    }
  }

  private final class InvalidStdoutUniversal(stdout: ToolMiddlewareOutputHandle) extends UniversalToolMiddleware {
    def invoke(
      invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
      Future.successful(Right(ToolMiddlewareResult(None, Some(stdout))))
  }

  private final class StartAndReturn extends UniversalToolMiddleware {
    def invoke(
      invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] = {
      underlying.start(invocation.commandPath, invocation.input, invocation.stdin)
      Future.successful(Right(ToolMiddlewareResult(None, None)))
    }
  }

  private lazy val registered: Unit = {
    val universalHandle = UniversalToolMiddlewareHandle(
      ToolMiddlewareDescriptor(
        universalName,
        List("universal-alias"),
        Doc("universal summary", "universal description"),
        ToolMiddlewareScope.Universal,
        ToolMiddleware.noParametersSchema
      ),
      _ => Right(ToolMiddleware.NoParameters()),
      () => new ForwardingUniversal
    )
    ToolMiddlewareImplementationRuntime.registerUniversal(universalHandle)

    val parameterCodec = IntoSchema[NestedParameters]
    ToolMiddlewareImplementationRuntime.registerUniversal(
      UniversalToolMiddlewareHandle(
        ToolMiddlewareDescriptor(
          typedUniversalName,
          Nil,
          Doc.empty,
          ToolMiddlewareScope.Universal,
          parameterCodec.graph
        ),
        value => FromSchema[NestedParameters].fromValue(value.value).left.map(_.message).map(identity[Any]),
        () => new TypedUniversal
      )
    )

    val wire    = monomorphicTool.tryToTool.toOption.get
    val schema  = monomorphicTool.canonicalInputRecordSchema(0).toOption.get
    val binding = ToolMiddlewareMethodBinding(
      "invoke",
      Nil,
      expectsStdin = true,
      (_, underlying, context) => {
        MonomorphicCaptured.principal = context.principal
        MonomorphicCaptured.stdinPresent = context.stdin.nonEmpty
        underlying.invoke(
          List("backend"),
          TypedSchemaValue(schema, SchemaValue.RecordValue(context.fields.map(_.value))),
          context.stdin
        )
      }
    )
    val monomorphicHandle = MonomorphicToolMiddlewareHandle(
      _ =>
        Right(
          ToolMiddlewareDescriptor(
            monomorphicName,
            List("monomorphic-alias"),
            Doc("monomorphic summary", "monomorphic description"),
            ToolMiddlewareScope.Monomorphic(wire, Some(wire)),
            ToolMiddleware.noParametersSchema
          )
        ),
      _ => Right(monomorphicTool),
      _ => Right(monomorphicTool),
      () => (),
      List(binding)
    )
    ToolMiddlewareImplementationRuntime.registerMonomorphic(monomorphicHandle)

    val stdoutWire   = typedStdoutTool.tryToTool.toOption.get
    val stdoutSchema = typedStdoutTool.canonicalInputRecordSchema(0).toOption.get
    ToolMiddlewareImplementationRuntime.registerMonomorphic(
      MonomorphicToolMiddlewareHandle(
        _ =>
          Right(
            ToolMiddlewareDescriptor(
              typedStdoutName,
              Nil,
              Doc.empty,
              ToolMiddlewareScope.Monomorphic(stdoutWire, Some(stdoutWire)),
              ToolMiddleware.noParametersSchema
            )
          ),
        _ => Right(typedStdoutTool),
        _ => Right(typedStdoutTool),
        () => (),
        List(
          ToolMiddlewareMethodBinding(
            "invoke",
            Nil,
            expectsStdin = false,
            (_, underlying, context) =>
              ToolUnderlyingRuntime
                .runInfallible(
                  underlying,
                  Right(typedStdoutTool),
                  Nil,
                  Right(TypedSchemaValue(stdoutSchema, SchemaValue.RecordValue(context.fields.map(_.value)))),
                  None
                )
                .toMiddlewareResult
          )
        )
      )
    )
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("ToolMiddlewareGuestSpec")(
      test("discovers sorted descriptors and gets both scope shapes") {
        registered
        val discovered  = guest.discoverToolMiddlewares().asInstanceOf[js.Array[JsToolMiddleware]].toList
        val ours        = discovered.filter(m => m.name == monomorphicName || m.name == universalName)
        val monomorphic = guest.getToolMiddleware(monomorphicName).asInstanceOf[JsToolMiddleware]
        val universal   = guest.getToolMiddleware(universalName).asInstanceOf[JsToolMiddleware]
        assertTrue(
          discovered.map(_.name) == discovered.map(_.name).sorted,
          ours.map(_.name) == List(monomorphicName, universalName),
          ToolWireInterop.toolMiddlewareFromJs(monomorphic).scope.isInstanceOf[ToolMiddlewareScope.Monomorphic],
          ToolWireInterop.toolMiddlewareFromJs(universal).scope == ToolMiddlewareScope.Universal
        )
      },
      test("get rejects an unknown middleware with the wire error") {
        val error =
          try {
            guest.getToolMiddleware("guest-middleware-missing")
            throw new RuntimeException("expected getToolMiddleware to throw")
          } catch {
            case js.JavaScriptException(value) => value.asInstanceOf[js.Dynamic]
          }
        assertTrue(
          error.tag.asInstanceOf[String] == "invalid-tool-name",
          error.selectDynamic("val").asInstanceOf[String] == "guest-middleware-missing"
        )
      },
      test("monomorphic dispatch decodes context and forwards the wrapped resource") {
        registered
        var wrappedPath: List[String]   = Nil
        var wrappedStdin: Boolean       = false
        var forwardedStdinValue: js.Any = js.undefined
        val stdin                       = js.Dynamic.global
          .eval("(async function* () { yield 7; })()")
          .asInstanceOf[JsWasiInputStream]
        val underlying = wrapped { (path, _, forwardedStdin) =>
          wrappedPath = path.toList
          wrappedStdin = !js.isUndefined(forwardedStdin)
          forwardedStdinValue = forwardedStdin
          resolved(JsInvocationResult(js.undefined, js.undefined))
        }
        for {
          result <- fromPromise(
                      invoke(
                        monomorphicName,
                        monomorphicTool.toolName,
                        toolToJs(monomorphicTool),
                        js.Array[String](),
                        monomorphicInput,
                        stdin,
                        underlying
                      )
                    )
        } yield assertTrue(
          resultOf(result).isEmpty,
          wrappedPath == List("backend"),
          wrappedStdin,
          forwardedStdinValue.asInstanceOf[js.Object] eq stdin,
          MonomorphicCaptured.stdinPresent,
          MonomorphicCaptured.principal == Principal.Anonymous
        )
      },
      test("universal dispatch roundtrips metadata, input, result, and principal") {
        registered
        val oidcPrincipal = js.Dynamic.literal(
          "tag" -> "oidc",
          "val" -> js.Dynamic.literal(
            "sub"               -> "subject-37",
            "issuer"            -> "https://issuer.example",
            "claims"            -> "{\"role\":\"admin\"}",
            "email"             -> "scala@example.com",
            "name"              -> "Scala Middleware",
            "emailVerified"     -> true,
            "givenName"         -> "Scala",
            "familyName"        -> "Middleware",
            "picture"           -> "https://example.com/picture.png",
            "preferredUsername" -> "scala-middleware"
          )
        )
        val expectedPrincipal = Principal.Oidc(
          sub = "subject-37",
          issuer = "https://issuer.example",
          claims = "{\"role\":\"admin\"}",
          email = Some("scala@example.com"),
          name = Some("Scala Middleware"),
          emailVerified = Some(true),
          givenName = Some("Scala"),
          familyName = Some("Middleware"),
          picture = Some("https://example.com/picture.png"),
          preferredUsername = Some("scala-middleware")
        )
        val metadata   = toolToJs(universalTool)
        val underlying = wrapped { (_, value, _) =>
          resolved(
            js.Dynamic.literal("result" -> value).asInstanceOf[JsInvocationResult]
          )
        }
        for {
          result <- fromPromise(
                      invoke(
                        universalName,
                        universalTool.toolName,
                        metadata,
                        js.Array("run"),
                        input("payload"),
                        noStdin,
                        underlying,
                        oidcPrincipal
                      )
                    )
        } yield assertTrue(
          resultOf(result).map(SchemaWireInterop.typedFromJs).contains(typed("payload")),
          UniversalCaptured.toolName == universalTool.toolName,
          UniversalCaptured.toolMetadata.contains(universalTool.tryToTool.toOption.get),
          UniversalCaptured.principal == expectedPrincipal
        )
      },
      test("declared nested parameters are decoded distinctly for successive invocations") {
        registered
        TypedUniversalCaptured.parameters = Nil
        val underlying                          = wrapped((_, value, _) => resolved(JsInvocationResult(value, js.undefined)))
        def parameters(value: NestedParameters) =
          SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(IntoSchema[NestedParameters].toTyped(value)))
        for {
          _ <- fromPromise(
                 invokeWithParameters(
                   typedUniversalName,
                   universalTool.toolName,
                   toolToJs(universalTool),
                   parameters(NestedParameters("first", NestedParameter(List(1, 2)))),
                   js.Array("run"),
                   input("one"),
                   noStdin,
                   underlying,
                   anonymous
                 )
               )
          _ <- fromPromise(
                 invokeWithParameters(
                   typedUniversalName,
                   universalTool.toolName,
                   toolToJs(universalTool),
                   parameters(NestedParameters("second", NestedParameter(List(8, 13)))),
                   js.Array("run"),
                   input("two"),
                   noStdin,
                   underlying,
                   anonymous
                 )
               )
        } yield assertTrue(
          TypedUniversalCaptured.parameters == List(
            NestedParameters("first", NestedParameter(List(1, 2))),
            NestedParameters("second", NestedParameter(List(8, 13)))
          )
        )
      },
      test("monomorphic empty parameters reject a non-record value with the correct schema") {
        registered
        var calls      = 0
        val underlying = wrapped { (_, value, _) => calls += 1; resolved(JsInvocationResult(value, js.undefined)) }
        val malformed  =
          TypedSchemaValue(ToolMiddleware.noParametersSchema, SchemaValue.StringValue("not-an-empty-record"))
        rejectionOf(
          invokeWithParameters(
            monomorphicName,
            monomorphicTool.toolName,
            toolToJs(monomorphicTool),
            SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(malformed)),
            js.Array[String](),
            monomorphicInput,
            noStdin,
            underlying,
            anonymous
          )
        ).map(error =>
          assertTrue(error.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "invalid-input", calls == 0)
        )
      },
      test("typed parameters reject wrong schema and wrong value type") {
        registered
        val codec                            = IntoSchema[NestedParameters]
        val valid                            = codec.toTyped(NestedParameters("valid", NestedParameter(List(1))))
        val wrongValue                       = valid.copy(value = SchemaValue.StringValue("not-a-tuple"))
        val underlying                       = wrapped((_, value, _) => resolved(JsInvocationResult(value, js.undefined)))
        def encoded(value: TypedSchemaValue) =
          SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(value))
        def call(parameters: JsTypedSchemaValue) =
          rejectionOf(
            invokeWithParameters(
              typedUniversalName,
              universalTool.toolName,
              toolToJs(universalTool),
              parameters,
              js.Array("run"),
              input("payload"),
              noStdin,
              underlying,
              anonymous
            )
          )
        for {
          wrongSchema <- call(input("wrong-schema"))
          wrongType   <- call(encoded(wrongValue))
        } yield assertTrue(
          wrongSchema.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "invalid-input",
          wrongType.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "invalid-input"
        )
      },
      test("wrapped declared errors preserve all protocol and custom tags") {
        registered
        val errors = List(
          WitToolError.InvalidToolName("missing"),
          WitToolError.InvalidCommandPath(List("bad", "path")),
          WitToolError.InvalidInput("bad input"),
          WitToolError.ConstraintViolation("constraint"),
          WitToolError.InvalidResult("bad result"),
          WitToolError.CustomError(WitCustomToolError("failure", typed("custom")))
        )
        ZIO
          .foreach(errors) { expected =>
            val underlying = wrapped((_, _, _) => js.Promise.reject(ToolWireInterop.toolErrorToJs(expected)))
            rejectionOf(
              invoke(
                universalName,
                universalTool.toolName,
                toolToJs(universalTool),
                js.Array("run"),
                input("payload"),
                noStdin,
                underlying
              )
            ).map(actual => ToolWireInterop.toolErrorFromJs(actual.asInstanceOf[JsToolError]))
          }
          .map(actual => assertTrue(actual == errors))
      },
      test("wrapped stdout resource is returned without replacement") {
        registered
        val stdout = js.Dynamic.global
          .eval("(async function* () { yield 17; yield 23; })()")
          .asInstanceOf[JsWasiOutputStream]
        val underlying = wrapped((_, _, _) => resolved(JsInvocationResult(js.undefined, stdout)))
        for {
          result <- fromPromise(
                      invoke(
                        universalName,
                        universalTool.toolName,
                        toolToJs(universalTool),
                        js.Array("run"),
                        input("payload"),
                        noStdin,
                        underlying
                      )
                    )
          stdoutResult = stdoutOf(result)
          iterator     = stdoutResult.asyncIterator()
          first       <- fromPromise(iterator.next())
          second      <- fromPromise(iterator.next())
        } yield assertTrue(
          stdoutResult eq stdout,
          first.value == 17,
          second.value == 23
        )
      },
      test("monomorphic typed projection validates stdout carried by JS admission") {
        registered
        val stdout = js.Dynamic.global
          .eval("(async function* () { yield 41; })()")
          .asInstanceOf[JsWasiOutputStream]
        val underlying = wrapped((_, _, _) => resolved(JsInvocationResult(js.undefined, stdout)))
        fromPromise(
          invoke(
            typedStdoutName,
            typedStdoutTool.toolName,
            toolToJs(typedStdoutTool),
            js.Array[String](),
            monomorphicInput,
            noStdin,
            underlying
          )
        ).map(result => assertTrue(stdoutOf(result) eq stdout))
      },
      test("dropping an unobserved pending invocation disposes immediately without starting get") {
        registered
        val middlewareName = "guest-middleware-drop-pending"
        ToolMiddlewareImplementationRuntime.registerUniversal(
          UniversalToolMiddlewareHandle(
            ToolMiddlewareDescriptor(
              middlewareName,
              Nil,
              Doc.empty,
              ToolMiddlewareScope.Universal,
              ToolMiddleware.noParametersSchema
            ),
            _ => Right(ToolMiddleware.NoParameters()),
            () => new StartAndReturn
          )
        )
        var gets                 = 0
        var disposals            = 0
        var cancellations        = 0
        var complete: () => Unit = null
        val getPromise           =
          new js.Promise[js.UndefOr[JsTypedSchemaValue]]((resolve, _) => complete = () => resolve(js.undefined))
        val observer = js.Dynamic.literal(
          "get"    -> js.Any.fromFunction0 { () => gets += 1; getPromise },
          "cancel" -> js.Any.fromFunction0(() => cancellations += 1)
        )
        js.Dynamic.global.Reflect.applyDynamic("set")(
          observer,
          js.Dynamic.global.Symbol.selectDynamic("dispose"),
          js.Any.fromFunction0 { () =>
            disposals += 1
          }
        )
        val underlying = js.Dynamic
          .literal(
            "invoke" -> js.Any.fromFunction3((_: js.Array[String], _: JsTypedSchemaValue, _: js.Any) =>
              js.Promise.resolve(js.Tuple2(observer.asInstanceOf[JsUnderlyingInvokeResult], js.undefined))
            )
          )
          .asInstanceOf[JsUnderlyingTool]
        for {
          _ <- fromPromise(
                 invoke(
                   middlewareName,
                   universalTool.toolName,
                   toolToJs(universalTool),
                   js.Array("run"),
                   input("payload"),
                   noStdin,
                   underlying
                 )
               )
          before = disposals
          _      = {
            complete()
          }
          _ <- ZIO.fromFuture(_ => FutureInterop.fromPromise(js.Promise.resolve(())))
        } yield assertTrue(before == 1, disposals == 1, cancellations == 0, gets == 0)
      },
      test("wrapped stdout is pumped through the host writer with backpressure") {
        registered
        val stdout = js.Dynamic.global
          .eval(
            "(async function* () { yield { tag: 'ok', val: new Uint8Array(70000) }; yield { tag: 'ok', val: new Uint8Array(70000) }; })()"
          )
          .asInstanceOf[JsWasiOutputStream]
        val underlying = wrapped((_, _, _) => resolved(JsInvocationResult(js.undefined, stdout)))
        var writes     = 0
        var active     = 0
        var maxActive  = 0
        var bytes      = 0
        var finishes   = 0
        val writer     = js.Dynamic
          .literal(
            "write" -> js.Any.fromFunction1 { (chunk: js.typedarray.Uint8Array) =>
              writes += 1
              active += 1
              maxActive = math.max(maxActive, active)
              bytes += chunk.length
              js.Promise.resolve(()).`then`[Unit](_ => active -= 1)
            },
            "finish" -> js.Any.fromFunction0 { () =>
              finishes += 1
              js.Promise.resolve(())
            },
            "fail" -> js.Any.fromFunction1((_: js.Any) => js.Promise.reject(new RuntimeException("unexpected failure")))
          )
          .asInstanceOf[ToolHostApi.RawToolStdoutWriter]
        fromPromise(
          invoke(
            universalName,
            universalTool.toolName,
            toolToJs(universalTool),
            js.Array("run"),
            input("payload"),
            noStdin,
            underlying,
            stdout = writer
          )
        ).map(result =>
          assertTrue(
            js.isUndefined(result.asInstanceOf[js.Dynamic].selectDynamic("stdout")),
            writes == 2,
            bytes == 140000,
            maxActive == 1,
            finishes == 1
          )
        )
      },
      test("invalid final stdout is rejected and closed") {
        registered
        val middlewareName = "guest-middleware-invalid-final-stdout"
        val stdout         = new CountingNonJsOutput
        ToolMiddlewareImplementationRuntime.registerUniversal(
          UniversalToolMiddlewareHandle(
            ToolMiddlewareDescriptor(
              middlewareName,
              Nil,
              Doc.empty,
              ToolMiddlewareScope.Universal,
              ToolMiddleware.noParametersSchema
            ),
            _ => Right(ToolMiddleware.NoParameters()),
            () => new InvalidStdoutUniversal(stdout)
          )
        )
        val underlying = wrapped((_, _, _) => resolved(JsInvocationResult(js.undefined, js.undefined)))
        fromPromise(
          invoke(
            middlewareName,
            universalTool.toolName,
            toolToJs(universalTool),
            js.Array("run"),
            input("payload"),
            noStdin,
            underlying
          )
        ).either.map { outcome =>
          val invalidResult = outcome.left.exists {
            case js.JavaScriptException(rejection) =>
              val error = rejection.asInstanceOf[js.Dynamic]
              error.selectDynamic("tag").asInstanceOf[String] == "invalid-result" &&
              error.selectDynamic("val").asInstanceOf[String] ==
                "tool middleware returned a non-JS tool stdout stream"
            case _ => false
          }
          assertTrue(
            invalidResult,
            stdout.closeCount == 1
          )
        }
      },
      test("unknown invoke and malformed inputs reject with protocol errors") {
        registered
        val success = wrapped((_, _, _) => resolved(JsInvocationResult(js.undefined, js.undefined)))
        for {
          missing <- rejectionOf(
                       invoke(
                         "guest-middleware-missing-invoke",
                         universalTool.toolName,
                         toolToJs(universalTool),
                         js.Array[String](),
                         input("payload"),
                         noStdin,
                         success
                       )
                     )
          malformedInput <- rejectionOf(
                              invoke(
                                universalName,
                                universalTool.toolName,
                                toolToJs(universalTool),
                                js.Array[String](),
                                js.Dynamic
                                  .literal("graph" -> js.Dynamic.literal())
                                  .asInstanceOf[JsTypedSchemaValue],
                                noStdin,
                                success
                              )
                            )
          malformedMetadata <- rejectionOf(
                                 invoke(
                                   universalName,
                                   universalTool.toolName,
                                   js.Dynamic.literal().asInstanceOf[JsTool],
                                   js.Array[String](),
                                   input("payload"),
                                   noStdin,
                                   success
                                 )
                               )
        } yield assertTrue(
          missing.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "invalid-tool-name",
          malformedInput.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "invalid-input",
          malformedMetadata.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "invalid-input"
        )
      },
      test("user promise failures remain unhandled failures") {
        registered
        val failure    = new js.Error("user middleware failure")
        val underlying = wrapped((_, _, _) => js.Promise.reject(failure))
        rejectionOf(
          invoke(
            universalName,
            universalTool.toolName,
            toolToJs(universalTool),
            js.Array[String](),
            input("payload"),
            noStdin,
            underlying
          )
        ).map(actual => assertTrue(actual.asInstanceOf[js.Object] eq failure))
      }
    ) @@ TestAspect.sequential
}
