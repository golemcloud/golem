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

package golem.tool

import golem.schema.{FromSchema, SchemaEncodeError, SchemaValue, TypedSchemaValue}

import scala.concurrent.Future

/** @internal Used by Golem-generated typed tool projections. */
sealed trait ToolDeclaredErrorDecoder[+E]
object ToolDeclaredErrorDecoder {
  case object NoDeclaredErrors                                                    extends ToolDeclaredErrorDecoder[Nothing]
  final case class DeclaredErrors[E](decode: NamedToolError => Either[String, E]) extends ToolDeclaredErrorDecoder[E]
}

/** @internal Used by Golem-generated typed tool projections. */
final case class PreparedToolCall[In, E, A](
  commandPath: List[String],
  input: Either[ToolInvokeError[Nothing], TypedSchemaValue],
  stdin: Option[In],
  errors: ToolDeclaredErrorDecoder[E],
  decodeValue: Option[TypedSchemaValue] => Either[String, A]
)

/**
 * @internal
 *   Shared input and result preparation used by generated projections.
 */
object ToolCallPreparation {
  def encodeParams(
    build: => List[(String, SchemaValue)]
  ): Either[ToolInvokeError[Nothing], List[(String, SchemaValue)]] =
    try Right(build)
    catch {
      case error: SchemaEncodeError =>
        Left(ToolInvokeError.InvalidInput(s"failed to encode tool parameter: ${error.message}"))
    }

  def prepareInput(
    descriptor: Either[ToolBuildError, ExtendedToolType],
    commandPath: List[String],
    inheritedPrefix: List[CanonicalInputValue],
    params: Either[ToolInvokeError[Nothing], List[(String, SchemaValue)]]
  ): Either[ToolInvokeError[Nothing], TypedSchemaValue] =
    descriptor match {
      case Left(error) =>
        Left(ToolInvokeError.InvalidResult(s"tool descriptor build failed: ${error.message}"))
      case Right(tool) =>
        tool.commandIndexByPath(commandPath) match {
          case None    => Left(ToolInvokeError.InvalidCommandPath(commandPath))
          case Some(_) =>
            params.flatMap { values =>
              val allValues = inheritedPrefix.map(value => value.name -> value.value) ++ values
              val input     = ToolClientRuntime.buildInputFromModel(
                ToolClientRuntime.staticInputModel(descriptor, commandPath),
                allValues
              )
              input.left.map(error => ToolInvokeError.InvalidInput(toolErrorMessage(error)))
            }
        }
    }

  def decodeUnit(value: Option[TypedSchemaValue]): Either[String, Unit] =
    value match {
      case None    => Right(())
      case Some(_) => Left("tool result unexpectedly contained a value")
    }

  def decodeValue[A](
    value: Option[TypedSchemaValue],
    from: FromSchema[A],
    expected: golem.schema.SchemaGraph
  ): Either[String, A] =
    value match {
      case None                                                                  => Left("tool result did not contain a value")
      case Some(result) if !ToolGraphs.schemaShapesMatch(result.graph, expected) =>
        Left("tool result schema does not match the generated client's expected result schema")
      case Some(result) => from.fromValue(result.value).left.map(_.message)
    }

  private def toolErrorMessage(error: ToolError[Nothing]): String =
    error match {
      case ToolError.Rpc(rpc)                  => rpc.message
      case ToolError.Tool(_)                   => "unexpected typed tool error"
      case ToolError.UnknownToolError(name, _) => s"unexpected tool error `$name`"
    }
}

/**
 * @internal
 *   Transport and failure policy used by generated typed tool projections.
 */
trait ToolCallBackend {
  type Stdin
  type Awaited[+E, +A]
  type StartedNoStdout[+E, +A]
  type StartedStdoutOnly[+E]
  type StartedValueStdout[+E, +A]

  def awaitNoStdout[E, A](call: PreparedToolCall[Stdin, E, A]): Awaited[E, A]
  def startNoStdout[E, A](call: PreparedToolCall[Stdin, E, A]): StartedNoStdout[E, A]
  def startStdoutOnly[E](call: PreparedToolCall[Stdin, E, Unit]): StartedStdoutOnly[E]
  def startValueStdout[E, A](call: PreparedToolCall[Stdin, E, A]): StartedValueStdout[E, A]
}

/** @internal Ambient typed tool-call policy over an injected RPC transport. */
final class AmbientToolCallBackend(val transport: ToolRpcTransport) extends ToolCallBackend {
  type Stdin                      = ToolInputStream
  type Awaited[+E, +A]            = Future[Either[ToolError[E], A]]
  type StartedNoStdout[+E, +A]    = Future[Either[ToolError[E], A]]
  type StartedStdoutOnly[+E]      = Either[ToolError[E], ToolInvocation[E, Unit]]
  type StartedValueStdout[+E, +A] = Either[ToolError[E], ToolInvocation[E, A]]

  def awaitNoStdout[E, A](
    call: PreparedToolCall[ToolInputStream, E, A]
  ): Future[Either[ToolError[E], A]] = {
    val result = call.errors match {
      case ToolDeclaredErrorDecoder.NoDeclaredErrors =>
        ToolClientRuntime.runInfallible(
          transport,
          call.commandPath,
          call.input.left.map(ambientError),
          call.stdin
        )
      case ToolDeclaredErrorDecoder.DeclaredErrors(decode) =>
        ToolClientRuntime.run(
          transport,
          call.commandPath,
          call.input.left.map(ambientError),
          call.stdin,
          decode
        )
    }
    ToolClientRuntime.complete(result)(decodeAmbient(call))
  }

  def startNoStdout[E, A](
    call: PreparedToolCall[ToolInputStream, E, A]
  ): Future[Either[ToolError[E], A]] =
    awaitNoStdout(call)

  def startStdoutOnly[E](
    call: PreparedToolCall[ToolInputStream, E, Unit]
  ): Either[ToolError[E], ToolInvocation[E, Unit]] =
    startAmbient(call)

  def startValueStdout[E, A](
    call: PreparedToolCall[ToolInputStream, E, A]
  ): Either[ToolError[E], ToolInvocation[E, A]] =
    startAmbient(call)

  private def startAmbient[E, A](
    call: PreparedToolCall[ToolInputStream, E, A]
  ): Either[ToolError[E], ToolInvocation[E, A]] = {
    val decodeError = call.errors match {
      case ToolDeclaredErrorDecoder.NoDeclaredErrors =>
        (_: NamedToolError) => Left("unexpected remote tool error")
      case ToolDeclaredErrorDecoder.DeclaredErrors(decode) => decode
    }
    ToolClientRuntime.start(
      transport,
      call.commandPath,
      call.input.left.map(ambientError),
      call.stdin,
      decodeError
    )(decodeAmbient(call))
  }

  private def decodeAmbient[E, A](
    call: PreparedToolCall[ToolInputStream, E, A]
  )(result: ToolInvokeResult): Either[ToolError[E], A] =
    call.decodeValue(result.result).left.map(message => ToolError.Rpc(RpcError.Protocol(message)))

  private def ambientError(error: ToolInvokeError[Nothing]): ToolError[Nothing] =
    ToolError.Rpc(RpcError.Protocol(error match {
      case ToolInvokeError.InvalidInput(message)  => message
      case ToolInvokeError.InvalidResult(message) => message
      case other                                  => ToolCallBackend.errorMessage(other)
    }))
}

/** @internal Invocation-scoped typed underlying policy. */
final class UnderlyingToolCallBackend(
  val underlying: RawToolUnderlying,
  val descriptor: Either[ToolBuildError, ExtendedToolType]
) extends ToolCallBackend {
  type Stdin                      = ToolMiddlewareInputHandle
  type Awaited[+E, +A]            = Future[Either[ToolInvokeError[E], A]]
  type StartedNoStdout[+E, +A]    = ToolUnderlyingInvocation[E, A]
  type StartedStdoutOnly[+E]      = ToolUnderlyingInvocation[E, ToolMiddlewareOutputHandle]
  type StartedValueStdout[+E, +A] = ToolUnderlyingInvocation[E, (A, ToolMiddlewareOutputHandle)]

  def awaitNoStdout[E, A](
    call: PreparedToolCall[ToolMiddlewareInputHandle, E, A]
  ): Future[Either[ToolInvokeError[E], A]] =
    startNoStdout(call).toMiddlewareResult

  def startNoStdout[E, A](
    call: PreparedToolCall[ToolMiddlewareInputHandle, E, A]
  ): ToolUnderlyingInvocation[E, A] =
    complete(
      start(call),
      result =>
        if (result.stdout.isDefined) Left("tool result unexpectedly contained stdout stream")
        else call.decodeValue(result.result)
    )

  def startStdoutOnly[E](
    call: PreparedToolCall[ToolMiddlewareInputHandle, E, Unit]
  ): ToolUnderlyingInvocation[E, ToolMiddlewareOutputHandle] =
    complete(
      start(call),
      result =>
        for {
          _      <- call.decodeValue(result.result)
          stdout <- result.stdout.toRight("tool result did not contain declared stdout stream")
        } yield stdout
    )

  def startValueStdout[E, A](
    call: PreparedToolCall[ToolMiddlewareInputHandle, E, A]
  ): ToolUnderlyingInvocation[E, (A, ToolMiddlewareOutputHandle)] =
    complete(
      start(call),
      result =>
        for {
          value  <- call.decodeValue(result.result)
          stdout <- result.stdout.toRight("tool result did not contain declared stdout stream")
        } yield (value, stdout)
    )

  private def start[E, A](
    call: PreparedToolCall[ToolMiddlewareInputHandle, E, A]
  ): ToolUnderlyingInvocation[E, ToolMiddlewareResult] =
    call.errors match {
      case ToolDeclaredErrorDecoder.NoDeclaredErrors =>
        ToolUnderlyingRuntime.runInfallible(
          underlying,
          descriptor,
          call.commandPath,
          call.input,
          call.stdin
        )
      case ToolDeclaredErrorDecoder.DeclaredErrors(decode) =>
        ToolUnderlyingRuntime.run(
          underlying,
          descriptor,
          call.commandPath,
          call.input,
          call.stdin,
          decode
        )
    }

  private def complete[E, A, B](
    call: ToolUnderlyingInvocation[E, ToolMiddlewareResult],
    decode: ToolMiddlewareResult => Either[String, B]
  ): ToolUnderlyingInvocation[E, B] =
    ToolUnderlyingRuntime.complete(call)(result =>
      decode(result).left.map(message => ToolError.Rpc(RpcError.Protocol(message)))
    )
}

private object ToolCallBackend {
  def errorMessage(error: ToolInvokeError[Nothing]): String =
    error match {
      case ToolInvokeError.InvalidToolName(name)        => s"invalid tool name `$name`"
      case ToolInvokeError.InvalidCommandPath(path)     => s"invalid command path `${path.mkString(" ")}`"
      case ToolInvokeError.InvalidInput(message)        => s"invalid input: $message"
      case ToolInvokeError.ConstraintViolation(message) => s"constraint violation: $message"
      case ToolInvokeError.InvalidResult(message)       => s"invalid result: $message"
      case ToolInvokeError.ProtocolError(message)       => s"protocol error: $message"
      case ToolInvokeError.Denied(message)              => s"denied: $message"
      case ToolInvokeError.InternalError(message)       => s"internal error: $message"
      case ToolInvokeError.Cancelled                    => "cancelled"
      case ToolInvokeError.ResourceExhausted(message)   => s"resource exhausted: $message"
      case ToolInvokeError.Tool(_)                      => "custom error"
      case ToolInvokeError.UnknownToolError(name, _)    => s"custom error `$name`"
    }
}
