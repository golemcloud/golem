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
import golem.schema.{SchemaValue, TypedSchemaValue}
import golem.schema.wire.{SchemaWire, WitTypedSchemaValue}
import golem.tool._
import golem.tool.wire.WitToolError
import golem.{FutureInterop, Principal}
import zio.test._
import zio.ZIO

import scala.concurrent.Future
import scala.scalajs.js

object ToolMiddlewareGuestSpec extends ZIOSpecDefault {
  import ToolTestFixtures._

  private val universalName   = "guest-middleware-universal"
  private val monomorphicName = "guest-middleware-monomorphic"
  private val universalTool   = richTool("guest-middleware-tool")
  private val monomorphicTool = echoTool("guest-middleware-echo")
  private val anonymous       = js.Dynamic.literal("tag" -> "anonymous")
  private val noStdin: js.Any = js.undefined.asInstanceOf[js.Any]

  private def guest: js.Dynamic = ToolMiddlewareGuest.golemTool010ToolMiddlewareGuest

  private def typed(value: String): WitTypedSchemaValue =
    SchemaWire.typedSchemaValueToWit(TypedSchemaValue(strGraph, SchemaValue.StringValue(value)))

  private def input(value: String): JsTypedSchemaValue =
    SchemaWireInterop.typedToJs(typed(value))

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
      .literal("invoke" -> js.Any.fromFunction3(invoke))
      .asInstanceOf[JsUnderlyingTool]

  private def invoke(
    middlewareName: String,
    toolName: String,
    metadata: JsTool,
    commandPath: js.Array[String],
    invocationInput: JsTypedSchemaValue,
    stdin: js.Any,
    underlying: JsUnderlyingTool,
    principal: js.Dynamic = anonymous
  ): js.Promise[JsInvocationResult] =
    guest
      .invokeToolMiddleware(
        middlewareName,
        toolName,
        metadata,
        commandPath,
        invocationInput,
        stdin,
        principal,
        underlying
      )
      .asInstanceOf[js.Promise[JsInvocationResult]]

  private final class ForwardingUniversal extends UniversalToolMiddleware {
    def invoke(
      invocation: UniversalToolMiddlewareInvocation,
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
      invocation: UniversalToolMiddlewareInvocation,
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
      Future.successful(Right(ToolMiddlewareResult(None, Some(stdout))))
  }

  private lazy val registered: Unit = {
    val universalHandle = UniversalToolMiddlewareHandle(
      ToolMiddlewareDescriptor(
        universalName,
        List("universal-alias"),
        Doc("universal summary", "universal description"),
        ToolMiddlewareScope.Universal
      ),
      () => new ForwardingUniversal
    )
    ToolMiddlewareImplementationRuntime.registerUniversal(universalHandle)

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
            ToolMiddlewareScope.Monomorphic(wire, Some(wire))
          )
        ),
      _ => Right(monomorphicTool),
      _ => Right(monomorphicTool),
      () => (),
      List(binding)
    )
    ToolMiddlewareImplementationRuntime.registerMonomorphic(monomorphicHandle)
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
      test("wrapped declared errors preserve all protocol and custom tags") {
        registered
        val errors = List(
          WitToolError.InvalidToolName("missing"),
          WitToolError.InvalidCommandPath(List("bad", "path")),
          WitToolError.InvalidInput("bad input"),
          WitToolError.ConstraintViolation("constraint"),
          WitToolError.InvalidResult("bad result"),
          WitToolError.CustomError(typed("custom"))
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
              ToolMiddlewareScope.Universal
            ),
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
