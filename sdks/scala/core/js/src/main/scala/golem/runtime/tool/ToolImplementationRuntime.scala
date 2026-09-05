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
import golem.host.js.tool.{JsByteStreamIterator, JsWasiInputStream, JsWasiOutputStream}
import golem.runtime.tool.host.ToolHostApi
import golem.schema.wire.SchemaWire
import golem.tool._
import golem.tool.wire.WitToolError

import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.JSConverters._

/** The stdin handle of a JS-guest tool invocation. */
final class JsToolInputStream(val underlying: ToolHostApi.RawByteStream) extends ToolInputStream {
  private lazy val iterator                                                   = underlying.asyncIterator()
  private var cancellation: Option[Future[Unit]]                              = None
  override def read(): Future[Either[ByteStreamFailure, Option[Array[Byte]]]] =
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

  override def cancel(): Future[Unit] =
    synchronized {
      cancellation.getOrElse {
        val result =
          try FutureInterop.fromPromise(iterator.returnIterator()).map(_ => ())(ToolInvokerRuntime.executionContext)
          catch {
            case error: Throwable => Future.failed(error)
          }
        cancellation = Some(result)
        result
      }
    }

  override private[golem] def close(): Future[Unit] =
    cancel().recover { case _ => () }(ToolInvokerRuntime.executionContext)
}

/** The stdout handle of a JS-guest tool invocation. */
final class JsToolOutputStream(val underlying: ToolHostApi.RawToolStdoutWriter) extends ToolOutputStream {
  private var terminal: Option[Future[Either[StreamWriteError, Unit]]] = None

  override def write(bytes: Array[Byte]): Future[Either[StreamWriteError, Unit]] =
    if (bytes.isEmpty) Future.successful(Right(()))
    else call(underlying.write(js.typedarray.Uint8Array.from(bytes.map(_.toShort).toJSArray)))
  override def finish(): Future[Either[StreamWriteError, Unit]] =
    selectTerminal(underlying.finish())
  override def fail(reason: ByteStreamFailure): Future[Either[StreamWriteError, Unit]] =
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

  override private[golem] def close(): Future[Unit] = finishInvocation()
}
object JsToolOutputStream {
  def decodeFailure(value: js.Dynamic): ByteStreamFailure = value.tag.asInstanceOf[String] match {
    case "cancelled"          => ByteStreamFailure.Cancelled
    case "abandoned"          => ByteStreamFailure.Abandoned
    case "resource-exhausted" => ByteStreamFailure.ResourceExhausted
    case "failed"             => ByteStreamFailure.Failed(value.`val`.asInstanceOf[String])
    case other                => ByteStreamFailure.Failed(s"unknown byte-stream failure: $other")
  }
}

/** Adapts the middleware ABI's legacy `stream<u8>` stdin. */
final class JsMiddlewareInputStream(val underlying: JsWasiInputStream) extends ToolMiddlewareInputHandle {
  private val lifecycle = new JsMiddlewareStreamLifecycle(() => underlying.asyncIterator())

  override private[golem] def close(): Future[Unit] = lifecycle.close()
}

/** Adapts the middleware ABI's legacy result-carried stdout stream. */
final class JsMiddlewareOutputStream(val underlying: JsWasiOutputStream) extends ToolMiddlewareOutputHandle {
  private val lifecycle = new JsMiddlewareStreamLifecycle(() => underlying.asyncIterator())

  override private[golem] def close(): Future[Unit] = lifecycle.close()
}

private final class JsMiddlewareStreamLifecycle(iterator: () => JsByteStreamIterator) {
  private implicit val ec: scala.concurrent.ExecutionContext =
    scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

  private var closed: Option[Future[Unit]] = None

  def close(): Future[Unit] =
    closed.getOrElse {
      val result =
        try {
          val rawIterator = iterator()
          val returnFn    = rawIterator.asInstanceOf[js.Dynamic].selectDynamic("return")
          if (js.typeOf(returnFn) == "function")
            FutureInterop
              .fromPromise(
                returnFn
                  .applyDynamic("call")(rawIterator)
                  .asInstanceOf[js.Promise[js.Any]]
              )
              .map(_ => ())
              .recover { case _ => () }
          else Future.successful(())
        } catch {
          case _: Throwable => Future.successful(())
        }
      closed = Some(result)
      result
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
      val decoded =
        try Right(SchemaWire.typedSchemaValueFromWit(wireInput))
        catch {
          case t: Throwable =>
            Left(ToolInvokeError.InvalidInput(s"malformed invocation input: ${String.valueOf(t.getMessage)}"))
        }
      val invoked = decoded match {
        case Left(error)  => Future.successful(Left(error))
        case Right(input) =>
          val env     = new JsToolInvokeEnv(stdout)
          val handler = ToolInvokerRuntime.handler(tool, handle, env)
          handler
            .invoke(
              commandPath,
              input,
              stdin,
              principal
            )
      }
      invoked.map {
        case Right(result) =>
          Right(
            ToolInvocationResult(
              result.result.map(SchemaWire.typedSchemaValueToWit)
            )
          )
        case Left(error) => Left(errorToWire(error))
      }
    }

  private[tool] def errorToWire(error: ToolInvokeError[golem.schema.TypedSchemaValue]): WitToolError =
    ToolInvokeError.toWire(error)

  private[tool] def errorFromWire(error: WitToolError): ToolInvokeError[golem.schema.TypedSchemaValue] =
    ToolInvokeError.fromWire(error)

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
          ): Future[Either[ToolInvokeError[golem.schema.TypedSchemaValue], ToolInvokeResult]] =
            val forwardedStdin = stdin match {
              case Some(stream: JsToolInputStream) => Right(Some(stream: ToolInputStream))
              case None                            => Right(None)
              case Some(_)                         => Left(ToolInvokeError.InvalidInput("unsupported nested stdin stream"))
            }
            forwardedStdin match {
              case Left(error)  => Future.successful(Left(error))
              case Right(value) =>
                registryInvoker(
                  commandPath,
                  SchemaWire.typedSchemaValueToWit(input),
                  value,
                  stdout.collect { case stream: JsToolOutputStream => stream: ToolOutputStream },
                  principal
                ).map {
                  case Right(result) =>
                    Right(
                      ToolInvokeResult(
                        result.result.map(SchemaWire.typedSchemaValueFromWit),
                        None
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
