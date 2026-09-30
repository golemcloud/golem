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
            body.copy(
              stdout = Some(StreamSpec(Doc.empty, List("application/octet-stream"), required = true)),
              stderr = Some(StreamSpec(Doc.empty, List("application/octet-stream"), required = true))
            )
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

  private def stderrOf(value: JsInvocationResult): JsWasiOutputStream =
    value.asInstanceOf[js.Dynamic].selectDynamic("stderr").asInstanceOf[JsWasiOutputStream]

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
                js.Tuple3(
                  js.Dynamic
                    .literal(
                      "get"    -> js.Any.fromFunction0(() => js.Promise.resolve(result.result)),
                      "cancel" -> js.Any.fromFunction0(() => ())
                    )
                    .asInstanceOf[JsUnderlyingInvokeResult],
                  result.stdout.asInstanceOf[js.UndefOr[JsWasiOutputStream]],
                  result.stderr.asInstanceOf[js.UndefOr[JsWasiOutputStream]]
                )
              )(ToolInvokerRuntime.executionContext)
          )
        )
      )
      .asInstanceOf[JsUnderlyingTool]

  private def admitted(
    result: js.Promise[js.UndefOr[JsTypedSchemaValue]],
    stdout: js.UndefOr[JsWasiOutputStream],
    stderr: js.UndefOr[JsWasiOutputStream],
    dispose: () => Unit = () => ()
  ): JsUnderlyingTool = {
    val observer = js.Dynamic.literal(
      "get"    -> js.Any.fromFunction0(() => result),
      "cancel" -> js.Any.fromFunction0(() => ())
    )
    js.Dynamic.global.Reflect.applyDynamic("set")(
      observer,
      js.Dynamic.global.Symbol.selectDynamic("dispose"),
      js.Any.fromFunction0(dispose)
    )
    js.Dynamic
      .literal(
        "invoke" -> js.Any.fromFunction3((_: js.Array[String], _: JsTypedSchemaValue, _: js.Any) =>
          js.Promise.resolve(
            js.Tuple3(observer.asInstanceOf[JsUnderlyingInvokeResult], stdout, stderr)
          )
        )
      )
      .asInstanceOf[JsUnderlyingTool]
  }

  private def outputWriter(
    write: js.typedarray.Uint8Array => js.Promise[Unit],
    finish: () => js.Promise[Unit],
    fail: js.Any => js.Promise[Unit]
  ): ToolHostApi.RawToolOutputWriter = {
    val writer = js.Dynamic.literal(
      "write"  -> js.Any.fromFunction1(write),
      "finish" -> js.Any.fromFunction0(finish),
      "fail"   -> js.Any.fromFunction1(fail)
    )
    js.Dynamic.global.Reflect.applyDynamic("set")(
      writer,
      js.Dynamic.global.Symbol.selectDynamic("dispose"),
      js.Any.fromFunction0(() => ())
    )
    writer.asInstanceOf[ToolHostApi.RawToolOutputWriter]
  }

  private def invoke(
    middlewareName: String,
    toolName: String,
    metadata: JsTool,
    commandPath: js.Array[String],
    invocationInput: JsTypedSchemaValue,
    stdin: js.Any,
    underlying: JsUnderlyingTool,
    principal: js.Dynamic = anonymous,
    stdout: js.UndefOr[ToolHostApi.RawToolOutputWriter] = js.undefined,
    stderr: js.UndefOr[ToolHostApi.RawToolOutputWriter] = js.undefined
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
      stdout,
      stderr
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
    stdout: js.UndefOr[ToolHostApi.RawToolOutputWriter] = js.undefined,
    stderr: js.UndefOr[ToolHostApi.RawToolOutputWriter] = js.undefined
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
        stderr,
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
      Future.successful(Right(ToolMiddlewareResult(None, Some(stdout), None)))
  }

  private final class StartAndReturn extends UniversalToolMiddleware {
    def invoke(
      invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] = {
      underlying.start(invocation.commandPath, invocation.input, invocation.stdin)
      Future.successful(Right(ToolMiddlewareResult(None, None, None)))
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
      test("writerless wrapped outputs are drained and omitted from the result") {
        registered
        val stdout = js.Dynamic.global
          .eval(
            """
              globalThis.__golemScalaStdoutReads = 0;
              ({ [Symbol.asyncIterator]() {
                const values = [
                  { tag: 'ok', val: new Uint8Array([17]) },
                  { tag: 'ok', val: new Uint8Array([23]) }
                ];
                let index = 0;
                return { async next() {
                  globalThis.__golemScalaStdoutReads++;
                  return index < values.length
                    ? { value: values[index++], done: false }
                    : { done: true };
                }};
              }})
            """
          )
          .asInstanceOf[JsWasiOutputStream]
        val stderr = js.Dynamic.global
          .eval(
            """
              globalThis.__golemScalaStderrReads = 0;
              ({ [Symbol.asyncIterator]() {
                const values = [
                  { tag: 'ok', val: new Uint8Array([29]) },
                  { tag: 'ok', val: new Uint8Array([31]) }
                ];
                let index = 0;
                return { async next() {
                  globalThis.__golemScalaStderrReads++;
                  return index < values.length
                    ? { value: values[index++], done: false }
                    : { done: true };
                }};
              }})
            """
          )
          .asInstanceOf[JsWasiOutputStream]
        val underlying = wrapped((_, _, _) =>
          resolved(
            JsInvocationResult(
              js.undefined,
              stdout,
              stderr
            )
          )
        )
        fromPromise(
          invoke(
            universalName,
            universalTool.toolName,
            toolToJs(universalTool),
            js.Array("run"),
            input("payload"),
            noStdin,
            underlying
          )
        ).map(result =>
          assertTrue(
            js.isUndefined(result.asInstanceOf[js.Dynamic].selectDynamic("stdout")),
            js.isUndefined(result.asInstanceOf[js.Dynamic].selectDynamic("stderr")),
            js.Dynamic.global.eval("globalThis.__golemScalaStdoutReads").asInstanceOf[Int] == 3,
            js.Dynamic.global.eval("globalThis.__golemScalaStderrReads").asInstanceOf[Int] == 3
          )
        )
      },
      test("admitted dual outputs are pumped before the structured result completes") {
        registered
        val state = js.Dynamic.global
          .eval(
            """
              (() => {
                let resolveResult;
                let completed = 0;
                const result = new Promise((resolve, reject) => {
                  resolveResult = resolve;
                  setTimeout(() => reject({ tag: 'protocol-error', val: 'outputs were not drained' }), 5000);
                });
                async function* output(byte) {
                  yield { tag: 'ok', val: new Uint8Array(65537).fill(byte) };
                  completed++;
                  if (completed === 2) resolveResult(undefined);
                }
                return { result, stdout: output(17), stderr: output(29) };
              })()
            """
          )
          .asInstanceOf[js.Dynamic]
        var stdoutBytes    = 0
        var stderrBytes    = 0
        var stdoutFinishes = 0
        var stderrFinishes = 0
        val stdoutWriter   = outputWriter(
          chunk => { stdoutBytes += chunk.length; js.Promise.resolve(()) },
          () => { stdoutFinishes += 1; js.Promise.resolve(()) },
          _ => js.Promise.reject(new RuntimeException("unexpected stdout failure"))
        )
        val stderrWriter = outputWriter(
          chunk => { stderrBytes += chunk.length; js.Promise.resolve(()) },
          () => { stderrFinishes += 1; js.Promise.resolve(()) },
          _ => js.Promise.reject(new RuntimeException("unexpected stderr failure"))
        )
        val underlying = admitted(
          state.result.asInstanceOf[js.Promise[js.UndefOr[JsTypedSchemaValue]]],
          state.stdout.asInstanceOf[JsWasiOutputStream],
          state.stderr.asInstanceOf[JsWasiOutputStream]
        )
        fromPromise(
          invoke(
            universalName,
            universalTool.toolName,
            toolToJs(universalTool),
            js.Array("run"),
            input("payload"),
            noStdin,
            underlying,
            stdout = stdoutWriter,
            stderr = stderrWriter
          )
        ).map(result =>
          assertTrue(
            resultOf(result).isEmpty,
            stdoutBytes == 65537,
            stderrBytes == 65537,
            stdoutFinishes == 1,
            stderrFinishes == 1
          )
        )
      },
      test("admitted output bytes settle before a declared structured error") {
        registered
        val state = js.Dynamic.global
          .eval(
            """
              (() => {
                let rejectResult;
                let completed = 0;
                const result = new Promise((_, reject) => { rejectResult = reject; });
                const state = { result, stdoutDrained: false, stderrDrained: false };
                async function* output(bytes, channel) {
                  yield { tag: 'ok', val: new Uint8Array(bytes) };
                  state[channel + 'Drained'] = true;
                  completed++;
                  if (completed === 2)
                    rejectResult({ tag: 'protocol-error', val: 'declared after output' });
                }
                state.stdout = output([3, 5], 'stdout');
                state.stderr = output([7, 11], 'stderr');
                return state;
              })()
            """
          )
          .asInstanceOf[js.Dynamic]
        var stdoutBytes    = 0
        var stderrBytes    = 0
        var stdoutFinishes = 0
        var stderrFinishes = 0
        val stdoutWriter   = outputWriter(
          chunk => { stdoutBytes += chunk.length; js.Promise.resolve(()) },
          () => { stdoutFinishes += 1; js.Promise.resolve(()) },
          _ => js.Promise.reject(new RuntimeException("unexpected stdout failure"))
        )
        val stderrWriter = outputWriter(
          chunk => { stderrBytes += chunk.length; js.Promise.resolve(()) },
          () => { stderrFinishes += 1; js.Promise.resolve(()) },
          _ => js.Promise.reject(new RuntimeException("unexpected stderr failure"))
        )
        val underlying = admitted(
          state.result.asInstanceOf[js.Promise[js.UndefOr[JsTypedSchemaValue]]],
          state.stdout.asInstanceOf[JsWasiOutputStream],
          state.stderr.asInstanceOf[JsWasiOutputStream]
        )
        rejectionOf(
          invoke(
            universalName,
            universalTool.toolName,
            toolToJs(universalTool),
            js.Array("run"),
            input("payload"),
            noStdin,
            underlying,
            stdout = stdoutWriter,
            stderr = stderrWriter
          )
        ).map { error =>
          assertTrue(
            error.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "invalid-result",
            error.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[String] ==
              "protocol error: declared after output",
            state.stdoutDrained.asInstanceOf[Boolean],
            state.stderrDrained.asInstanceOf[Boolean],
            stdoutBytes == 0,
            stderrBytes == 0,
            stdoutFinishes == 1,
            stderrFinishes == 1
          )
        }
      },
      test("one admitted output failure does not interrupt sibling settlement") {
        registered
        val stdout = js.Dynamic.global
          .eval("(async function* () { yield { tag: 'ok', val: new Uint8Array([13]) }; })()")
          .asInstanceOf[JsWasiOutputStream]
        val stderr = js.Dynamic.global
          .eval(
            "(async function* () { yield { tag: 'ok', val: new Uint8Array([17]) }; yield { tag: 'ok', val: new Uint8Array([19]) }; })()"
          )
          .asInstanceOf[JsWasiOutputStream]
        var stderrBytes    = 0
        var stderrFinishes = 0
        val stdoutWriter   = outputWriter(
          _ => js.Promise.reject(new RuntimeException("stdout write failed")),
          () => js.Promise.resolve(()),
          _ => js.Promise.resolve(())
        )
        val stderrWriter = outputWriter(
          chunk => { stderrBytes += chunk.length; js.Promise.resolve(()) },
          () => { stderrFinishes += 1; js.Promise.resolve(()) },
          _ => js.Promise.reject(new RuntimeException("unexpected stderr failure"))
        )
        val underlying = admitted(js.Promise.resolve(js.undefined), stdout, stderr)
        fromPromise(
          invoke(
            universalName,
            universalTool.toolName,
            toolToJs(universalTool),
            js.Array("run"),
            input("payload"),
            noStdin,
            underlying,
            stdout = stdoutWriter,
            stderr = stderrWriter
          )
        ).either.map(outcome => assertTrue(outcome.isLeft, stderrBytes == 2, stderrFinishes == 1))
      },
      test("sequential and overlapping children retain independent selectable outputs") {
        registered
        ZIO
          .foreach(List(false, true)) { overlapping =>
            val middlewareName = s"guest-middleware-select-${if overlapping then "overlap" else "sequential"}"
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
                () =>
                  new UniversalToolMiddleware {
                    def invoke(
                      invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
                      underlying: UniversalToolUnderlying
                    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] = {
                      val first = underlying.invoke(List("first"), invocation.input, invocation.stdin)
                      def select(
                        selected: Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult],
                        discarded: Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]
                      ) = selected match {
                        case Left(error) => Left(error)
                        case right       => discarded.fold(Left(_), _ => right)
                      }
                      if overlapping then {
                        val second = underlying.invoke(List("second"), invocation.input, None)
                        first
                          .zip(second)
                          .map { case (selected, discarded) => select(selected, discarded) }(
                            ToolInvokerRuntime.executionContext
                          )
                      } else
                        first.flatMap(selected =>
                          underlying
                            .invoke(List("second"), invocation.input, None)
                            .map(discarded => select(selected, discarded))(ToolInvokerRuntime.executionContext)
                        )(ToolInvokerRuntime.executionContext)
                    }
                  }
              )
            )
            var active     = 0
            var maxActive  = 0
            val underlying = js.Dynamic
              .literal(
                "invoke" -> js.Any.fromFunction3 { (path: js.Array[String], _: JsTypedSchemaValue, _: js.Any) =>
                  val byte   = if path.toList == List("first") then 61 else 67
                  val stream = js.Dynamic.global
                    .eval(
                      s"(async function* () { yield { tag: 'ok', val: new Uint8Array([$byte]) }; })()"
                    )
                    .asInstanceOf[JsWasiOutputStream]
                  val result = new js.Promise[js.UndefOr[JsTypedSchemaValue]]((resolve, _) => {
                    active += 1
                    maxActive = math.max(maxActive, active)
                    js.Dynamic.global.setTimeout(
                      js.Any.fromFunction0 { () =>
                        active -= 1
                        resolve(js.undefined)
                      },
                      5
                    )
                  })
                  admitted(result, stream, js.undefined).invoke(path, input("ignored"), js.undefined)
                }
              )
              .asInstanceOf[JsUnderlyingTool]
            var bytes    = Vector.empty[Int]
            var finishes = 0
            val writer   = outputWriter(
              chunk => { bytes ++= chunk.toArray.map(_ & 0xff); js.Promise.resolve(()) },
              () => { finishes += 1; js.Promise.resolve(()) },
              _ => js.Promise.reject(new RuntimeException("unexpected selected-output failure"))
            )
            fromPromise(
              invoke(
                middlewareName,
                universalTool.toolName,
                toolToJs(universalTool),
                js.Array("run"),
                input("payload"),
                noStdin,
                underlying,
                stdout = writer
              )
            ).map(_ =>
              assertTrue(
                bytes == Vector(61),
                finishes == 1,
                maxActive == (if overlapping then 2 else 1)
              )
            )
          }
          .map(results => results.reduce(_ && _))
      },
      test("monomorphic typed projection drains dual outputs before structured completion") {
        registered
        val state = js.Dynamic.global
          .eval(
            """
              (() => {
                let resolveResult;
                let completed = 0;
                const result = new Promise(resolve => { resolveResult = resolve; });
                const state = { result, stdoutDrained: false, stderrDrained: false };
                async function* output(bytes, channel) {
                  yield { tag: 'ok', val: new Uint8Array(bytes) };
                  state[channel + 'Drained'] = true;
                  completed++;
                  if (completed === 2) resolveResult(undefined);
                }
                state.stdout = output([41, 43], 'stdout');
                state.stderr = output([47, 53], 'stderr');
                return state;
              })()
            """
          )
          .asInstanceOf[js.Dynamic]
        val underlying = admitted(
          state.result.asInstanceOf[js.Promise[js.UndefOr[JsTypedSchemaValue]]],
          state.stdout.asInstanceOf[JsWasiOutputStream],
          state.stderr.asInstanceOf[JsWasiOutputStream]
        )
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
        ).map(result =>
          assertTrue(
            state.stdoutDrained.asInstanceOf[Boolean],
            state.stderrDrained.asInstanceOf[Boolean],
            js.isUndefined(result.asInstanceOf[js.Dynamic].selectDynamic("stdout")),
            js.isUndefined(result.asInstanceOf[js.Dynamic].selectDynamic("stderr"))
          )
        )
      },
      test("monomorphic typed projection waits for delayed output after an early result") {
        registered
        val state = js.Dynamic.global
          .eval(
            """
              (() => {
                const state = { stdoutDrained: false, stderrDrained: false };
                state.gate = new Promise(resolve => { state.release = resolve; });
                async function* output(bytes, channel) {
                  yield { tag: 'ok', val: new Uint8Array(bytes) };
                  await state.gate;
                  state[channel + 'Drained'] = true;
                }
                state.stdout = output([59], 'stdout');
                state.stderr = output([61], 'stderr');
                return state;
              })()
            """
          )
          .asInstanceOf[js.Dynamic]
        val underlying = admitted(
          js.Promise.resolve(js.undefined),
          state.stdout.asInstanceOf[JsWasiOutputStream],
          state.stderr.asInstanceOf[JsWasiOutputStream]
        )
        var settled = false
        val started = invoke(
          typedStdoutName,
          typedStdoutTool.toolName,
          toolToJs(typedStdoutTool),
          js.Array[String](),
          monomorphicInput,
          noStdin,
          underlying
        )
        started.`then`[Unit](_ => settled = true)
        for {
          _     <- fromPromise(js.Promise.resolve(()))
          before = settled
          _      = state.release.asInstanceOf[js.Function0[Unit]]()
          _     <- fromPromise(started)
        } yield assertTrue(
          !before,
          settled,
          state.stdoutDrained.asInstanceOf[Boolean],
          state.stderrDrained.asInstanceOf[Boolean]
        )
      },
      test("monomorphic typed projection preserves a buffered output iterator failure") {
        registered
        val output = js.Dynamic.global
          .eval(
            "(async function* () { yield { tag: 'ok', val: new Uint8Array([67]) }; throw new Error('buffered source failed'); })()"
          )
          .asInstanceOf[JsWasiOutputStream]
        val underlying = admitted(
          js.Promise.resolve(js.undefined),
          output,
          js.Dynamic.global
            .eval("(async function* () {})()")
            .asInstanceOf[JsWasiOutputStream]
        )
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
        ).flip.map(error => assertTrue(error.getMessage.contains("buffered source failed")))
      },
      test("closing a buffered output closes its source") {
        val state = js.Dynamic.global
          .eval(
            """
              (() => {
                const state = { returned: false };
                const iterator = {
                  next: () => new Promise(() => {}),
                  return: () => {
                    state.returned = true;
                    return Promise.resolve({ done: true });
                  }
                };
                state.output = { [Symbol.asyncIterator]: () => iterator };
                return state;
              })()
            """
          )
          .asInstanceOf[js.Dynamic]
        val buffered = JsMiddlewareOutputStream.buffered(
          state.output.asInstanceOf[JsWasiOutputStream]
        )
        ZIO.fromFuture(_ => buffered.close()).map { _ =>
          assertTrue(state.returned.asInstanceOf[Boolean])
        }
      },
      test("monomorphic typed projection drains output bytes preceding a custom error") {
        registered
        val customError = js.Dynamic.literal(
          "tag" -> "tool-error",
          "val" -> ToolWireInterop.toolErrorToJs(
            WitToolError.CustomError(WitCustomToolError("failure", typed("typed failure")))
          )
        )
        js.Dynamic.global.Reflect.applyDynamic("set")(
          js.Dynamic.global.eval("globalThis"),
          "__golemScalaTypedCustomError",
          customError
        )
        val state = js.Dynamic.global
          .eval(
            """
              (() => {
                let rejectResult;
                let completed = 0;
                const result = new Promise((_, reject) => { rejectResult = reject; });
                const state = { result, stdoutDrained: false, stderrDrained: false };
                async function* output(bytes, channel) {
                  yield { tag: 'ok', val: new Uint8Array(bytes) };
                  state[channel + 'Drained'] = true;
                  completed++;
                  if (completed === 2) rejectResult(globalThis.__golemScalaTypedCustomError);
                }
                state.stdout = output([89], 'stdout');
                state.stderr = output([97], 'stderr');
                return state;
              })()
            """
          )
          .asInstanceOf[js.Dynamic]
        val underlying = admitted(
          state.result.asInstanceOf[js.Promise[js.UndefOr[JsTypedSchemaValue]]],
          state.stdout.asInstanceOf[JsWasiOutputStream],
          state.stderr.asInstanceOf[JsWasiOutputStream]
        )
        rejectionOf(
          invoke(
            typedStdoutName,
            typedStdoutTool.toolName,
            toolToJs(typedStdoutTool),
            js.Array[String](),
            monomorphicInput,
            noStdin,
            underlying
          )
        ).map(error =>
          assertTrue(
            state.stdoutDrained.asInstanceOf[Boolean],
            state.stderrDrained.asInstanceOf[Boolean],
            error.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "invalid-result"
          )
        )
      },
      test("completed convenience calls dispose their observer exactly once") {
        registered
        var disposals  = 0
        val underlying = admitted(
          js.Promise.resolve(js.undefined),
          js.undefined,
          js.undefined,
          () => disposals += 1
        )
        fromPromise(
          invoke(
            universalName,
            universalTool.toolName,
            toolToJs(universalTool),
            js.Array("run"),
            input("payload"),
            noStdin,
            underlying
          )
        ).map(_ => assertTrue(disposals == 1))
      },
      test("early handler return disposes a late convenience observer and drains abandoned output") {
        registered
        val middlewareName = "guest-middleware-early-convenience-return"
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
            () =>
              new UniversalToolMiddleware {
                def invoke(
                  invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
                  underlying: UniversalToolUnderlying
                ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] = {
                  underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)
                  Future.successful(Right(ToolMiddlewareResult(None, None, None)))
                }
              }
          )
        )
        val state = js.Dynamic.global
          .eval(
            """
              (() => {
                const state = { drained: false };
                state.gate = new Promise(resolve => { state.release = resolve; });
                state.done = new Promise(resolve => { state.markDone = resolve; });
                state.output = (async function* () {
                  await state.gate;
                  yield { tag: 'ok', val: new Uint8Array([101, 103]) };
                  state.drained = true;
                  state.markDone();
                })();
                return state;
              })()
            """
          )
          .asInstanceOf[js.Dynamic]
        var complete: () => Unit = () => ()
        val terminal             =
          new js.Promise[js.UndefOr[JsTypedSchemaValue]]((resolve, _) => complete = () => resolve(js.undefined))
        var disposals  = 0
        val underlying = admitted(
          terminal,
          state.output.asInstanceOf[JsWasiOutputStream],
          js.undefined,
          () => disposals += 1
        )
        val started = invoke(
          middlewareName,
          universalTool.toolName,
          toolToJs(universalTool),
          js.Array("run"),
          input("payload"),
          noStdin,
          underlying
        )
        for {
          _     <- fromPromise(js.Promise.resolve(()))
          before = (disposals, state.drained.asInstanceOf[Boolean])
          _      = state.release.asInstanceOf[js.Function0[Unit]]()
          _      = complete()
          _     <- fromPromise(started)
          _     <- fromPromise(state.done.asInstanceOf[js.Promise[Unit]])
        } yield assertTrue(
          before == ((0, false)),
          state.drained.asInstanceOf[Boolean],
          disposals == 1
        )
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
              js.Promise.resolve(
                js.Tuple3(observer.asInstanceOf[JsUnderlyingInvokeResult], js.undefined, js.undefined)
              )
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
            "(async function* () { yield { tag: 'ok', val: new Uint8Array([7]) }; yield { tag: 'ok', val: new Uint8Array([9]) }; })()"
          )
          .asInstanceOf[JsWasiOutputStream]
        val underlying  = wrapped((_, _, _) => resolved(JsInvocationResult(js.undefined, stdout)))
        var writes      = 0
        var active      = 0
        var maxActive   = 0
        var bytes       = 0
        var finishes    = 0
        val writerValue = js.Dynamic.literal(
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
        js.Dynamic.global.Reflect.applyDynamic("set")(
          writerValue,
          js.Dynamic.global.Symbol.selectDynamic("dispose"),
          js.Any.fromFunction0(() => ())
        )
        val writer = writerValue.asInstanceOf[ToolHostApi.RawToolOutputWriter]
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
            bytes == 2,
            maxActive == 1,
            finishes == 1
          )
        )
      },
      test("tagged output failures propagate without terminating the sibling channel") {
        registered
        val stdout = js.Dynamic.global
          .eval(
            """
              (async function* () {
                yield { tag: 'ok', val: new Uint8Array([71, 73]) };
                yield { tag: 'err', val: { tag: 'failed', val: 'stdout failed' } };
              })()
            """
          )
          .asInstanceOf[JsWasiOutputStream]
        val stderr = js.Dynamic.global
          .eval("(async function* () { yield { tag: 'ok', val: new Uint8Array([79, 83]) }; })()")
          .asInstanceOf[JsWasiOutputStream]
        val underlying     = wrapped((_, _, _) => resolved(JsInvocationResult(js.undefined, stdout, stderr)))
        var stdoutBytes    = 0
        var stdoutFailures = Vector.empty[String]
        var stderrBytes    = 0
        var stderrFinishes = 0
        val stdoutWriter   = outputWriter(
          chunk => { stdoutBytes += chunk.length; js.Promise.resolve(()) },
          () => js.Promise.reject(new RuntimeException("unexpected stdout finish")),
          reason => {
            stdoutFailures :+= reason.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[String]
            js.Promise.resolve(())
          }
        )
        val stderrWriter = outputWriter(
          chunk => { stderrBytes += chunk.length; js.Promise.resolve(()) },
          () => { stderrFinishes += 1; js.Promise.resolve(()) },
          _ => js.Promise.reject(new RuntimeException("unexpected stderr failure"))
        )
        fromPromise(
          invoke(
            universalName,
            universalTool.toolName,
            toolToJs(universalTool),
            js.Array("run"),
            input("payload"),
            noStdin,
            underlying,
            stdout = stdoutWriter,
            stderr = stderrWriter
          )
        ).map(_ =>
          assertTrue(
            stdoutBytes == 2,
            stdoutFailures == Vector("stdout failed"),
            stderrBytes == 2,
            stderrFinishes == 1
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
                "tool middleware returned a non-JS tool output stream"
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
