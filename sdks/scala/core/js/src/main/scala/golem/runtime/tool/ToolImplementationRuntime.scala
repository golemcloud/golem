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

import golem.FutureInterop
import golem.runtime.tool.host.ToolHostApi
import golem.schema.wire.SchemaWire
import golem.tool._
import golem.tool.wire.WitToolError

import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.JSConverters._

/** The stdin handle of a JS-guest tool invocation. */
final class JsToolInputStream(val underlying: ToolHostApi.RawByteStream) extends ToolInputStream {
  private val iterator                                               = underlying.asyncIterator()
  def read(): Future[Either[ByteStreamFailure, Option[Array[Byte]]]] =
    FutureInterop
      .fromPromise(iterator.next())
      .map { next =>
        if (next.done) Right(None)
        else {
          val item = next.value.asInstanceOf[js.Dynamic]
          item.tag.asInstanceOf[String] match {
            case "ok"  => Right(Some(item.`val`.asInstanceOf[js.typedarray.Uint8Array].toArray.map(_.toByte)))
            case "err" => Left(JsToolOutputStream.decodeFailure(item.`val`))
          }
        }
      }(ToolInvokerRuntime.executionContext)

  def cancel(): Future[Unit] =
    FutureInterop.fromPromise(iterator.returnIterator()).map(_ => ())(ToolInvokerRuntime.executionContext)
}

/** The stdout handle of a JS-guest tool invocation. */
final class JsToolOutputStream(val underlying: ToolHostApi.RawToolStdoutWriter) extends ToolOutputStream {
  private var terminal: Option[Future[Either[StreamWriteError, Unit]]] = None

  def write(bytes: Array[Byte]): Future[Either[StreamWriteError, Unit]] =
    call(underlying.write(js.typedarray.Uint8Array.from(bytes.map(_.toShort).toJSArray)))
  def finish(): Future[Either[StreamWriteError, Unit]] =
    selectTerminal(underlying.finish())
  def fail(reason: ByteStreamFailure): Future[Either[StreamWriteError, Unit]] =
    selectTerminal(underlying.fail(encodeFailure(reason)))

  private[tool] def finishInvocation(): Future[Unit] =
    terminal
      .getOrElse(finish())
      .flatMap {
        case Right(_)                         => Future.successful(())
        case Left(StreamWriteError.Closed(_)) => Future.successful(())
        case Left(error)                      =>
          Future.failed(new IllegalStateException(s"tool stdout terminal failed: $error"))
      }(ToolInvokerRuntime.executionContext)

  private def selectTerminal(
    promise: => js.Promise[Unit]
  ): Future[Either[StreamWriteError, Unit]] =
    terminal match {
      case Some(selected) => selected
      case None           =>
        val selected = call(promise)
        terminal = Some(selected)
        selected
    }

  private def call(promise: => js.Promise[Unit]): Future[Either[StreamWriteError, Unit]] =
    try
      FutureInterop
        .fromPromise(promise)
        .map(_ => Right(()): Either[StreamWriteError, Unit])(ToolInvokerRuntime.executionContext)
        .recoverWith { case error: js.JavaScriptException =>
          ToolHostApi
            .decodeStreamWriteError(error.exception)
            .fold(Future.failed[Either[StreamWriteError, Unit]](error))(value => Future.successful(Left(value)))
        }(ToolInvokerRuntime.executionContext)
    catch {
      case error: js.JavaScriptException =>
        ToolHostApi
          .decodeStreamWriteError(error.exception)
          .fold(Future.failed[Either[StreamWriteError, Unit]](error))(value => Future.successful(Left(value)))
      case error: Throwable => Future.failed(error)
    }

  private def encodeFailure(failure: ByteStreamFailure): js.Any = failure match {
    case ByteStreamFailure.Cancelled         => js.Dynamic.literal(tag = "cancelled")
    case ByteStreamFailure.Abandoned         => js.Dynamic.literal(tag = "abandoned")
    case ByteStreamFailure.ResourceExhausted => js.Dynamic.literal(tag = "resource-exhausted")
    case ByteStreamFailure.Failed(message)   => js.Dynamic.literal(tag = "failed", `val` = message)
  }
}
object JsToolOutputStream {
  def decodeFailure(value: js.Dynamic): ByteStreamFailure = value.tag.asInstanceOf[String] match {
    case "cancelled"          => ByteStreamFailure.Cancelled
    case "abandoned"          => ByteStreamFailure.Abandoned
    case "resource-exhausted" => ByteStreamFailure.ResourceExhausted
    case "failed"             => ByteStreamFailure.Failed(value.`val`.asInstanceOf[String])
  }
}

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
    ToolRegistry.registerInvoker(extended, adaptHandler(extended, handle))
  }

  private[tool] def adaptHandler(
    tool: ExtendedToolType,
    handle: ToolImplementationHandle
  ): ToolRegistry.ToolInvoker =
    (commandPath, wireInput, stdin, stdout, principal) => {
      val output  = stdout.map(new JsToolOutputStream(_))
      val decoded =
        try Right(SchemaWire.typedSchemaValueFromWit(wireInput))
        catch {
          case t: Throwable =>
            Left(ToolInvokeError.InvalidInput(s"malformed invocation input: ${String.valueOf(t.getMessage)}"))
        }
      val invoked = decoded match {
        case Left(error)  => Future.successful(Left(error))
        case Right(input) =>
          val env     = new JsToolInvokeEnv(output)
          val handler = ToolInvokerRuntime.handler(tool, handle, env)
          handler
            .invoke(
              commandPath,
              input,
              stdin.map(new JsToolInputStream(_)),
              principal
            )
      }
      invoked.flatMap { outcome =>
        val completed = output match {
          case Some(stream) => stream.finishInvocation()
          case None         => Future.successful(())
        }
        completed.map(_ =>
          outcome match {
            case Right(result) =>
              Right(
                ToolInvocationResult(
                  result.result.map(SchemaWire.typedSchemaValueToWit)
                )
              )
            case Left(error) => Left(errorToWire(error))
          }
        )
      }
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
   * the [[ToolRegistry]] and forwards the invocation's directional stream
   * capabilities.
   */
  private[golem] final class JsToolInvokeEnv(val stdout: Option[ToolOutputStream]) extends ToolInvokeEnv {

    def invokerFor(toolName: String): Option[ToolInvokeHandler] =
      ToolRegistry.getInvoker(toolName).map { registryInvoker =>
        new ToolInvokeHandler {
          def invoke(
            commandPath: List[String],
            input: golem.schema.TypedSchemaValue,
            stdin: Option[ToolInputStream],
            principal: golem.Principal
          ): Future[Either[ToolInvokeError, ToolInvokeResult]] =
            val rawStdin = stdin match {
              case Some(stream: JsToolInputStream) => Right(Some(stream.underlying))
              case None                            => Right(None)
              case Some(_)                         => Left(ToolInvokeError.InvalidInput("unsupported nested stdin stream"))
            }
            rawStdin match {
              case Left(error)  => Future.successful(Left(error))
              case Right(value) =>
                registryInvoker(
                  commandPath,
                  SchemaWire.typedSchemaValueToWit(input),
                  value,
                  stdout.collect { case stream: JsToolOutputStream => stream.underlying },
                  principal
                ).map {
                  case Right(result) =>
                    Right(
                      ToolInvokeResult(
                        result.result.map(SchemaWire.typedSchemaValueFromWit)
                      )
                    )
                  case Left(error) => Left(errorFromWire(error))
                }
            }
        }
      }

    def extendedToolFor(toolName: String): Option[ExtendedToolType] =
      ToolRegistry.getExtendedTool(toolName)
  }
}
