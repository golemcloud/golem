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

import golem.host.js.tool.{JsWasiInputStream, JsWasiOutputStream}
import golem.schema.wire.SchemaWire
import golem.tool._
import golem.tool.wire.WitToolError

import scala.concurrent.Future

/** The stdin handle of a JS-guest tool invocation. */
final class JsToolInputStream(val underlying: JsWasiInputStream) extends ToolInputStream

/** The stdout handle of a JS-guest tool invocation. */
final class JsToolOutputStream(val underlying: JsWasiOutputStream) extends ToolOutputStream

/**
 * Registers macro-generated tool implementations into the [[ToolRegistry]],
 * adapting the platform-neutral invocation surface to the registry's wire
 * types.
 */
private[golem] object ToolImplementationRuntime {

  private implicit val ec: scala.concurrent.ExecutionContext =
    ToolInvokerRuntime.executionContext

  def register(handle: ToolImplementationHandle): Unit = {
    val ctx      = new ToolBuildCtx
    val extended = handle.descriptor(ctx) match {
      case Right(tool) => tool
      case Left(error) =>
        throw new IllegalArgumentException(s"tool descriptor build failed: ${error.message}")
    }
    val handler = ToolInvokerRuntime.handler(extended, handle, JsToolInvokeEnv)
    ToolRegistry.registerInvoker(extended, adaptHandler(handler))
  }

  private def adaptHandler(handler: ToolInvokeHandler): ToolRegistry.ToolInvoker =
    (commandPath, wireInput, stdin, principal) => {
      val decoded =
        try Right(SchemaWire.typedSchemaValueFromWit(wireInput))
        catch {
          case t: Throwable =>
            Left(WitToolError.InvalidInput(s"malformed invocation input: ${String.valueOf(t.getMessage)}"))
        }
      decoded match {
        case Left(error)  => Future.successful(Left(error))
        case Right(input) =>
          handler
            .invoke(commandPath, input, stdin.map(new JsToolInputStream(_)), principal)
            .map {
              case Right(result) =>
                Right(
                  ToolInvocationResult(
                    result.result.map(SchemaWire.typedSchemaValueToWit),
                    result.stdout.map(jsStdout)
                  )
                )
              case Left(error) => Left(errorToWire(error))
            }
      }
    }

  private def jsStdout(stream: ToolOutputStream): JsWasiOutputStream =
    stream match {
      case js: JsToolOutputStream => js.underlying
      case other                  =>
        throw new IllegalStateException(
          s"unexpected non-JS tool stdout stream: ${other.getClass.getName}"
        )
    }

  private[tool] def errorToWire(error: ToolInvokeError): WitToolError =
    error match {
      case ToolInvokeError.InvalidToolName(name)    => WitToolError.InvalidToolName(name)
      case ToolInvokeError.InvalidCommandPath(path) => WitToolError.InvalidCommandPath(path)
      case ToolInvokeError.InvalidInput(message)    => WitToolError.InvalidInput(message)
      case ToolInvokeError.ConstraintViolation(m)   => WitToolError.ConstraintViolation(m)
      case ToolInvokeError.InvalidResult(message)   => WitToolError.InvalidResult(message)
      case ToolInvokeError.Custom(payload)          =>
        WitToolError.CustomError(SchemaWire.typedSchemaValueToWit(payload))
    }

  private[tool] def errorFromWire(error: WitToolError): ToolInvokeError =
    error match {
      case WitToolError.InvalidToolName(name)    => ToolInvokeError.InvalidToolName(name)
      case WitToolError.InvalidCommandPath(path) => ToolInvokeError.InvalidCommandPath(path)
      case WitToolError.InvalidInput(message)    => ToolInvokeError.InvalidInput(message)
      case WitToolError.ConstraintViolation(m)   => ToolInvokeError.ConstraintViolation(m)
      case WitToolError.InvalidResult(message)   => ToolInvokeError.InvalidResult(message)
      case WitToolError.CustomError(payload)     =>
        ToolInvokeError.Custom(SchemaWire.typedSchemaValueFromWit(payload))
    }

  /**
   * The JS-guest tool invocation environment: sibling tool lookup goes through
   * the [[ToolRegistry]]; stdout acquisition is not yet wired into the QuickJS
   * guest world.
   */
  private[golem] object JsToolInvokeEnv extends ToolInvokeEnv {

    def stdout(): ToolOutputStream =
      throw new UnsupportedOperationException(
        "tool stdout streams are not supported by the JS guest runtime yet"
      )

    def invokerFor(toolName: String): Option[ToolInvokeHandler] =
      ToolRegistry.getInvoker(toolName).map { registryInvoker =>
        new ToolInvokeHandler {
          def invoke(
            commandPath: List[String],
            input: golem.schema.TypedSchemaValue,
            stdin: Option[ToolInputStream],
            principal: golem.Principal
          ): Future[Either[ToolInvokeError, ToolInvokeResult]] =
            registryInvoker(
              commandPath,
              SchemaWire.typedSchemaValueToWit(input),
              stdin.map(_.asInstanceOf[JsToolInputStream].underlying),
              principal
            ).map {
              case Right(result) =>
                Right(
                  ToolInvokeResult(
                    result.result.map(SchemaWire.typedSchemaValueFromWit),
                    result.stdout.map(new JsToolOutputStream(_))
                  )
                )
              case Left(error) => Left(errorFromWire(error))
            }
        }
      }

    def extendedToolFor(toolName: String): Option[ExtendedToolType] =
      ToolRegistry.getExtendedTool(toolName)
  }
}
