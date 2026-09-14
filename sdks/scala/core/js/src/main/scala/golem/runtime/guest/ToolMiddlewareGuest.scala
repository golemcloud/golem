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
import golem.runtime.tool.{JsMiddlewareInputStream, JsMiddlewareOutputStream, ToolMiddlewareRegistry}
import golem.schema.wire.SchemaWire
import golem.tool._
import golem.tool.wire.WitToolError
import golem.FutureInterop

import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.scalajs.js.annotation.JSExportTopLevel

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
    commandPath: js.Array[String],
    input: JsTypedSchemaValue,
    stdin: js.UndefOr[JsWasiInputStream],
    principal: js.Dynamic,
    wrapped: JsUnderlyingTool
  ): js.Promise[JsInvocationResult] =
    ToolMiddlewareRegistry.getInvoker(middlewareName) match {
      case None          => rejectToolError(ToolInvokeError.InvalidToolName(middlewareName))
      case Some(invoker) =>
        decodeInvocation(toolMetadata, input, stdin, principal, wrapped) match {
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
                  commandPath.toList,
                  decoded.input,
                  decoded.stdin,
                  decoded.principal,
                  validateFinalStdout
                )
            }
            FutureInterop.toPromise(
              invocation.map {
                case Right(result) => resultToJs(result)
                case Left(error)   =>
                  throw js.JavaScriptException(
                    ToolWireInterop.toolErrorToJs(ToolInvokeError.toWire(error))
                  )
              }
            )
        }
    }

  private final case class DecodedInvocation(
    toolMetadata: golem.tool.wire.WitTool,
    input: golem.schema.TypedSchemaValue,
    stdin: Option[ToolMiddlewareInputHandle],
    principal: golem.Principal,
    wrapped: RawToolUnderlying
  )

  private def decodeInvocation(
    toolMetadata: JsTool,
    input: JsTypedSchemaValue,
    stdin: js.UndefOr[JsWasiInputStream],
    principal: js.Dynamic,
    wrapped: JsUnderlyingTool
  ): Either[ToolInvokeError.InvalidInput, DecodedInvocation] =
    try {
      Right(
        DecodedInvocation(
          ToolWireInterop.toolFromJs(toolMetadata),
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
      ): Future[Either[ToolInvokeError[golem.schema.TypedSchemaValue], ToolMiddlewareResult]] = {
        val call =
          try {
            val jsStdin = stdin.map {
              case stream: JsMiddlewareInputStream => stream.underlying
              case other                           =>
                throw new IllegalStateException(
                  s"unexpected non-JS tool stdin stream: ${other.getClass.getName}"
                )
            }.orUndefined
            FutureInterop.fromPromise(
              wrapped.invoke(
                commandPath.toJSArray,
                SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(input)),
                jsStdin
              )
            )
          } catch {
            case error: Throwable => Future.failed(error)
          }
        call
          .map(result => Right(resultFromJs(result)))
          .recoverWith { case error @ js.JavaScriptException(value) =>
            decodeToolError(value) match {
              case Some(declared) => Future.successful(Left(declared))
              case None           => Future.failed(error)
            }
          }
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

  @JSExportTopLevel("golemTool010ToolMiddlewareGuest")
  val golemTool010ToolMiddlewareGuest: js.Dynamic =
    js.Dynamic.literal(
      discoverToolMiddlewares = js.Any.fromFunction0(() => discoverToolMiddlewares()),
      getToolMiddleware = js.Any.fromFunction1((name: String) => getToolMiddleware(name)),
      invokeToolMiddleware = js.Any.fromFunction8(
        (
          middlewareName: String,
          toolName: String,
          toolMetadata: JsTool,
          commandPath: js.Array[String],
          input: JsTypedSchemaValue,
          stdin: js.UndefOr[JsWasiInputStream],
          principal: js.Dynamic,
          wrapped: JsUnderlyingTool
        ) =>
          invokeToolMiddleware(
            middlewareName,
            toolName,
            toolMetadata,
            commandPath,
            input,
            stdin,
            principal,
            wrapped
          )
      )
    )

  @JSExportTopLevel("toolMiddlewareGuest")
  val toolMiddlewareGuest: js.Dynamic = golemTool010ToolMiddlewareGuest
}
