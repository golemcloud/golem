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

package golem.runtime.guest

import golem.host.js.PrincipalConverter
import golem.host.js.schema.JsTypedSchemaValue
import golem.host.js.tool._
import golem.host.{SchemaWireInterop, ToolWireInterop}
import golem.runtime.tool.{
  JsMiddlewareInputStream,
  JsMiddlewareOutputStream,
  JsToolInputStream,
  JsToolOutputStream,
  ToolMiddlewareRegistry
}
import golem.runtime.tool.host.ToolHostApi
import golem.schema.wire.SchemaWire
import golem.tool._
import golem.tool.wire.WitToolError
import golem.FutureInterop

import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.JSConverters._

object ToolMiddlewareGuest {
  private implicit val ec: scala.concurrent.ExecutionContext =
    ToolInvokerRuntime.executionContext

  private val invalidStdoutMessage =
    "tool middleware returned a non-JS tool stdout stream"

  private val validateFinalStdout: ToolMiddlewareOutputHandle => Either[String, Unit] = {
    case _: JsMiddlewareOutputStream => Right(())
    case _                           => Left(invalidStdoutMessage)
  }

  private def rejectToolError[A](error: ToolInvokeError[golem.schema.TypedSchemaValue]): js.Promise[A] =
    js.Promise.reject(ToolWireInterop.toolErrorToJs(ToolInvokeError.toWire(error))).asInstanceOf[js.Promise[A]]

  private def discoverToolMiddlewares(): js.Array[JsToolMiddleware] =
    ToolMiddlewareRegistry.allMiddlewares.map(ToolWireInterop.toolMiddlewareToJs).toJSArray

  private def getToolMiddleware(name: String): JsToolMiddleware =
    ToolMiddlewareRegistry.getMiddleware(name) match {
      case Some(middleware) => ToolWireInterop.toolMiddlewareToJs(middleware)
      case None             =>
        throw js.JavaScriptException(
          ToolWireInterop.toolErrorToJs(WitToolError.InvalidToolName(name))
        )
    }

  private def invokeToolMiddleware(
    middlewareName: String,
    toolName: String,
    toolMetadata: JsTool,
    parameters: JsTypedSchemaValue,
    commandPath: js.Array[String],
    input: JsTypedSchemaValue,
    stdin: js.UndefOr[JsWasiInputStream],
    stdout: js.UndefOr[ToolHostApi.RawToolStdoutWriter],
    principal: js.Dynamic,
    wrapped: JsUnderlyingTool
  ): js.Promise[JsInvocationResult] =
    ToolMiddlewareRegistry.getInvoker(middlewareName) match {
      case None          => rejectToolError(ToolInvokeError.InvalidToolName(middlewareName))
      case Some(invoker) =>
        decodeInvocation(toolMetadata, parameters, input, stdin, principal, wrapped) match {
          case Left(error)    => rejectToolError(error)
          case Right(decoded) =>
            val invocation = invoker match {
              case ToolMiddlewareRegistry.ToolMiddlewareInvoker.Monomorphic(presented, _, handle) =>
                ToolMiddlewareInvokerRuntime.invoke(
                  presented,
                  handle,
                  decoded.wrapped,
                  toolName,
                  commandPath.toList,
                  decoded.input,
                  decoded.parameters,
                  decoded.stdin,
                  decoded.principal,
                  validateFinalStdout
                )
              case ToolMiddlewareRegistry.ToolMiddlewareInvoker.Universal(handle) =>
                UniversalToolMiddlewareInvokerRuntime.invoke(
                  handle,
                  decoded.wrapped,
                  toolName,
                  decoded.toolMetadata,
                  decoded.parameters,
                  commandPath.toList,
                  decoded.input,
                  decoded.stdin,
                  decoded.principal,
                  validateFinalStdout
                )
            }
            FutureInterop.toPromise(
              invocation.flatMap {
                case Right(result) => forwardStdout(result, stdout)
                case Left(error)   =>
                  Future.failed(
                    js.JavaScriptException(
                      ToolWireInterop.toolErrorToJs(ToolInvokeError.toWire(error))
                    )
                  )
              }
            )
        }
    }

  private final case class DecodedInvocation(
    toolMetadata: golem.tool.wire.WitTool,
    parameters: golem.schema.TypedSchemaValue,
    input: golem.schema.TypedSchemaValue,
    stdin: Option[ToolMiddlewareInputHandle],
    principal: golem.Principal,
    wrapped: RawToolUnderlying
  )

  private def decodeInvocation(
    toolMetadata: JsTool,
    parameters: JsTypedSchemaValue,
    input: JsTypedSchemaValue,
    stdin: js.UndefOr[JsWasiInputStream],
    principal: js.Dynamic,
    wrapped: JsUnderlyingTool
  ): Either[ToolInvokeError.InvalidInput, DecodedInvocation] =
    try {
      Right(
        DecodedInvocation(
          ToolWireInterop.toolFromJs(toolMetadata),
          SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(parameters)),
          SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(input)),
          stdin.toOption.map(new JsMiddlewareInputStream(_)),
          PrincipalConverter.fromJs(principal),
          rawUnderlying(wrapped)
        )
      )
    } catch {
      case error: Throwable =>
        Left(
          ToolInvokeError.InvalidInput(
            s"malformed tool middleware invocation: ${String.valueOf(error.getMessage)}"
          )
        )
    }

  private def rawUnderlying(wrapped: JsUnderlyingTool): RawToolUnderlying =
    new RawToolUnderlying {
      def invoke(
        commandPath: List[String],
        input: golem.schema.TypedSchemaValue,
        stdin: Option[ToolMiddlewareInputHandle]
      ): Future[Either[ToolInvokeError[golem.schema.TypedSchemaValue], ToolMiddlewareResult]] =
        start(commandPath, input, stdin).admission.flatMap { admission =>
          admission.result.map {
            case Right(value)                                     => Right(value.copy(stdout = admission.stdout))
            case Left(ToolUnderlyingError.Tool(error))            => Left(error)
            case Left(ToolUnderlyingError.ProtocolError(message)) => Left(ToolInvokeError.ProtocolError(message))
            case Left(ToolUnderlyingError.Denied(message))        => Left(ToolInvokeError.Denied(message))
            case Left(ToolUnderlyingError.InternalError(message)) => Left(ToolInvokeError.InternalError(message))
            case Left(ToolUnderlyingError.Cancelled)              =>
              Left(ToolInvokeError.Cancelled)
            case Left(ToolUnderlyingError.ResourceExhausted(message)) =>
              Left(ToolInvokeError.ResourceExhausted(message))
          }
        }

      override def start(
        commandPath: List[String],
        input: golem.schema.TypedSchemaValue,
        stdin: Option[ToolMiddlewareInputHandle]
      ): ToolUnderlyingInvocation[golem.schema.TypedSchemaValue, ToolMiddlewareResult] = {
        val admission =
          try {
            val jsStdin = stdin.map {
              case stream: JsMiddlewareInputStream => stream.underlying
              case other                           =>
                throw new IllegalStateException(
                  s"unexpected non-JS tool stdin stream: ${other.getClass.getName}"
                )
            }.orUndefined
            FutureInterop
              .fromPromise(
                wrapped.invoke(
                  commandPath.toJSArray,
                  SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(input)),
                  jsStdin
                )
              )
              .flatMap { started =>
                var observing                  = false
                var settled                    = false
                var dropRequested              = false
                var disposed                   = false
                def disposeWhenSettled(): Unit =
                  if ((!observing || settled) && dropRequested && !disposed) {
                    disposed = true
                    try disposeUnderlyingObserver(started._1)
                    catch { case _: Throwable => () }
                  }
                def terminal
                  : Future[Either[ToolUnderlyingError[golem.schema.TypedSchemaValue], ToolMiddlewareResult]] = {
                  if (disposed)
                    return Future.successful(
                      Left(ToolUnderlyingError.ProtocolError("underlying invocation observer was dropped"))
                    )
                  observing = true
                  FutureInterop
                    .fromPromise(started._1.get())
                    .map { value =>
                      Right(
                        ToolMiddlewareResult(
                          value.toOption.map(v => SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(v))),
                          None
                        )
                      )
                    }
                    .recoverWith { case error @ js.JavaScriptException(value) =>
                      decodeUnderlyingError(value) match {
                        case Some(declared) => Future.successful(Left(declared))
                        case None           => Future.failed(error)
                      }
                    }
                    .transform { outcome =>
                      settled = true
                      disposeWhenSettled()
                      outcome
                    }
                }
                Future.successful(
                  ToolUnderlyingAdmission(
                    started._2.toOption.map(new JsMiddlewareOutputStream(_)),
                    () => terminal,
                    () => if (!settled && !disposed) started._1.cancel(),
                    () => {
                      dropRequested = true
                      disposeWhenSettled()
                    }
                  )
                )
              }
          } catch {
            case error: Throwable => Future.failed(error)
          }
        ToolUnderlyingInvocation(admission)
      }
    }

  private def decodeUnderlyingError(value: Any): Option[ToolUnderlyingError[golem.schema.TypedSchemaValue]] =
    try
      value.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] match {
        case "tool-error"     => decodeToolError(value.asInstanceOf[js.Dynamic].`val`).map(ToolUnderlyingError.Tool(_))
        case "protocol-error" =>
          Some(ToolUnderlyingError.ProtocolError(value.asInstanceOf[js.Dynamic].`val`.asInstanceOf[String]))
        case "denied"         => Some(ToolUnderlyingError.Denied(value.asInstanceOf[js.Dynamic].`val`.asInstanceOf[String]))
        case "internal-error" =>
          Some(ToolUnderlyingError.InternalError(value.asInstanceOf[js.Dynamic].`val`.asInstanceOf[String]))
        case "cancelled"          => Some(ToolUnderlyingError.Cancelled)
        case "resource-exhausted" =>
          Some(ToolUnderlyingError.ResourceExhausted(value.asInstanceOf[js.Dynamic].`val`.asInstanceOf[String]))
        case _ => None
      }
    catch { case _: Throwable => None }

  private def disposeUnderlyingObserver(observer: JsUnderlyingInvokeResult): Unit = {
    val symbol  = js.Dynamic.global.Symbol.selectDynamic("dispose")
    val release = js.Dynamic.global.Reflect.applyDynamic("get")(observer, symbol)
    release.applyDynamic("call")(observer)
    ()
  }

  private def forwardStdout(
    result: ToolMiddlewareResult,
    supplied: js.UndefOr[ToolHostApi.RawToolStdoutWriter]
  ): Future[JsInvocationResult] = supplied.toOption match {
    case None         => Future.successful(resultToJs(result))
    case Some(writer) =>
      result.stdout match {
        case None =>
          new JsToolOutputStream(writer).finish().flatMap {
            case Right(_) =>
              Future.successful(
                JsInvocationResult(
                  result.result.map(v => SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(v))).orUndefined,
                  js.undefined
                )
              )
            case Left(error) => Future.failed(new IllegalStateException(s"middleware stdout finish failed: $error"))
          }
        case Some(stream: JsMiddlewareOutputStream) =>
          val source               = new JsToolInputStream(stream.underlying.asInstanceOf[ToolHostApi.RawByteStream])
          val target               = new JsToolOutputStream(writer)
          def loop(): Future[Unit] = source.read().flatMap {
            case Right(Some(bytes)) =>
              target.write(bytes).flatMap {
                case Right(_)    => loop()
                case Left(error) => Future.failed(new IllegalStateException(s"middleware stdout write failed: $error"))
              }
            case Right(None) =>
              target.finish().flatMap {
                case Right(_)    => Future.successful(())
                case Left(error) => Future.failed(new IllegalStateException(s"middleware stdout finish failed: $error"))
              }
            case Left(failure) =>
              target.fail(failure).flatMap {
                case Right(_)    => Future.successful(())
                case Left(error) =>
                  Future.failed(new IllegalStateException(s"middleware stdout failure forwarding failed: $error"))
              }
          }
          loop().map(_ =>
            JsInvocationResult(
              result.result.map(v => SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(v))).orUndefined,
              js.undefined
            )
          )
        case Some(other) =>
          Future.failed(new IllegalStateException(s"unexpected middleware stdout: ${other.getClass.getName}"))
      }
  }

  private def decodeToolError(value: Any): Option[ToolInvokeError[golem.schema.TypedSchemaValue]] =
    try {
      val tag = value.asInstanceOf[js.Dynamic].tag.asInstanceOf[String]
      if (
        tag == "invalid-tool-name" || tag == "invalid-command-path" ||
        tag == "invalid-input" || tag == "constraint-violation" ||
        tag == "invalid-result" || tag == "custom-error"
      )
        Some(
          ToolInvokeError.fromWire(
            ToolWireInterop.toolErrorFromJs(value.asInstanceOf[JsToolError])
          )
        )
      else None
    } catch {
      case _: Throwable => None
    }

  private def resultFromJs(result: JsInvocationResult): ToolMiddlewareResult =
    ToolMiddlewareResult(
      result.result.toOption.map(value => SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(value))),
      result.stdout.toOption.map(new JsMiddlewareOutputStream(_))
    )

  private def resultToJs(result: ToolMiddlewareResult): JsInvocationResult =
    JsInvocationResult(
      result.result
        .map(value => SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(value)))
        .orUndefined,
      result.stdout.map {
        case stream: JsMiddlewareOutputStream => stream.underlying
        case other                            =>
          throw new IllegalStateException(
            s"unexpected non-JS tool stdout stream: ${other.getClass.getName}"
          )
      }.orUndefined
    )

  def golemTool010ToolMiddlewareGuest: js.Dynamic =
    js.Dynamic.literal(
      discoverToolMiddlewares = js.Any.fromFunction0(() => discoverToolMiddlewares()),
      getToolMiddleware = js.Any.fromFunction1((name: String) => getToolMiddleware(name)),
      invokeToolMiddleware = js.Any.fromFunction10(
        (
          middlewareName: String,
          toolName: String,
          toolMetadata: JsTool,
          parameters: JsTypedSchemaValue,
          commandPath: js.Array[String],
          input: JsTypedSchemaValue,
          stdin: js.UndefOr[JsWasiInputStream],
          stdout: js.UndefOr[ToolHostApi.RawToolStdoutWriter],
          principal: js.Dynamic,
          wrapped: JsUnderlyingTool
        ) =>
          invokeToolMiddleware(
            middlewareName,
            toolName,
            toolMetadata,
            parameters,
            commandPath,
            input,
            stdin,
            stdout,
            principal,
            wrapped
          )
      )
    )

}
