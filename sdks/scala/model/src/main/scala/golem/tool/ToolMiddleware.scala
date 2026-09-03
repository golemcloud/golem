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

import golem.Principal
import golem.schema.{FromSchema, SchemaValue, TypedSchemaValue}
import golem.schema.validation.{ValueValidation, WellFormedness}
import golem.tool.wire.WitTool

import scala.collection.mutable
import scala.concurrent.{Future, Promise}
import scala.util.{Success, Try}

final case class ToolMiddlewareDescriptor(
  name: String,
  aliases: List[String],
  doc: Doc,
  scope: ToolMiddlewareScope
)

final case class ToolMiddlewareMethodBinding(
  methodName: String,
  commandPath: List[String],
  expectsStdin: Boolean,
  run: (
    Any,
    RawToolUnderlying,
    ToolMiddlewareInvocationContext
  ) => Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]]
)

final case class ToolMiddlewareResult(
  result: Option[TypedSchemaValue],
  stdout: Option[ToolMiddlewareOutputHandle]
)

sealed trait ToolMiddlewareParamDecoder extends Product with Serializable
object ToolMiddlewareParamDecoder {
  final case class Field(
    canonicalName: String,
    decode: CanonicalInputValue => Either[String, Any]
  ) extends ToolMiddlewareParamDecoder

  case object PrincipalParam extends ToolMiddlewareParamDecoder
  case object StdinParam     extends ToolMiddlewareParamDecoder
}

final case class MonomorphicToolMiddlewareHandle(
  descriptor: ToolBuildCtx => Either[ToolBuildError, ToolMiddlewareDescriptor],
  presented: ToolBuildCtx => Either[ToolBuildError, ExtendedToolType],
  expected: ToolBuildCtx => Either[ToolBuildError, ExtendedToolType],
  newInstance: () => Any,
  bindings: List[ToolMiddlewareMethodBinding]
)

final case class UniversalToolMiddlewareHandle(
  descriptor: ToolMiddlewareDescriptor,
  newInstance: () => UniversalToolMiddleware
)

sealed trait ToolMiddlewareScope extends Product with Serializable
object ToolMiddlewareScope {
  final case class Monomorphic(
    presented: WitTool,
    expected: Option[WitTool]
  ) extends ToolMiddlewareScope

  case object Universal extends ToolMiddlewareScope
}

trait RawToolUnderlying {
  def invoke(
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolMiddlewareInputHandle]
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]]
}

final class ToolMiddlewareInvocationContext(
  val fields: List[CanonicalInputValue],
  val stdin: Option[ToolMiddlewareInputHandle],
  val principal: Principal
)

object ToolMiddlewareInvokerRuntime {
  private implicit val ec: scala.concurrent.ExecutionContext =
    ToolInvokerRuntime.executionContext

  def invoke(
    presented: ExtendedToolType,
    handle: MonomorphicToolMiddlewareHandle,
    wrapped: RawToolUnderlying,
    toolName: String,
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolMiddlewareInputHandle],
    principal: Principal,
    validateFinalStdout: ToolMiddlewareOutputHandle => Either[String, Unit] = _ => Right(())
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
    ToolMiddlewareOwnershipRuntime.withInvocationScopedUnderlying(wrapped, stdin, validateFinalStdout) { underlying =>
      val instance = handle.newInstance()
      if (toolName != presented.toolName)
        failed(ToolInvokeError.InvalidToolName(toolName))
      else
        presented.commandIndexByPath(commandPath) match {
          case None               => failed(ToolInvokeError.InvalidCommandPath(commandPath))
          case Some(commandIndex) =>
            validateInput(presented, commandIndex, input) match {
              case Left(error) => failed(error)
              case Right(_)    =>
                presented.decodeCanonicalInputRecord(commandIndex, input.value) match {
                  case Left(error)   => failed(ToolInvokeError.InvalidInput(error.message))
                  case Right(fields) =>
                    handle.bindings
                      .find(binding => presented.commandIndexByPath(binding.commandPath).contains(commandIndex)) match {
                      case None                                                     => failed(ToolInvokeError.InvalidCommandPath(commandPath))
                      case Some(binding) if stdin.nonEmpty && !binding.expectsStdin =>
                        failed(ToolInvokeError.InvalidInput("tool invocation contained unexpected stdin stream"))
                      case Some(binding) =>
                        binding
                          .run(
                            instance,
                            underlying,
                            new ToolMiddlewareInvocationContext(fields, stdin, principal)
                          )
                          .map(outcome =>
                            ToolMiddlewareOwnershipRuntime.validateFinal(underlying, outcome) { tracked =>
                              validateOutcome(presented, commandIndex, tracked)
                            }
                          )
                    }
                }
            }
        }
    }

  private def validateInput(
    presented: ExtendedToolType,
    commandIndex: Int,
    input: TypedSchemaValue
  ): Either[ToolInvokeError.InvalidInput, Unit] =
    WellFormedness.validateGraph(input.graph) match {
      case Left(errors) =>
        Left(ToolInvokeError.InvalidInput(errors.map(_.message).mkString("; ")))
      case Right(_) =>
        presented.canonicalInputRecordSchema(commandIndex) match {
          case Left(error)                                                             => Left(ToolInvokeError.InvalidInput(error.message))
          case Right(expected) if !ToolGraphs.schemaShapesMatch(input.graph, expected) =>
            Left(ToolInvokeError.InvalidInput("tool invocation input schema does not match the presented command"))
          case Right(expected) =>
            ValueValidation.validateValue(expected, expected.root, input.value) match {
              case Left(errors) =>
                Left(ToolInvokeError.InvalidInput(errors.map(_.message).mkString("; ")))
              case Right(_) => Right(())
            }
        }
    }

  def decodeArgs(
    ctx: ToolMiddlewareInvocationContext,
    decoders: List[ToolMiddlewareParamDecoder]
  ): Either[ToolInvokeError[Nothing], Vector[Any]] = {
    val args = Vector.newBuilder[Any]
    val it   = decoders.iterator
    while (it.hasNext) {
      it.next() match {
        case ToolMiddlewareParamDecoder.Field(canonicalName, decode) =>
          ctx.fields.find(_.name == canonicalName) match {
            case None =>
              return Left(ToolInvokeError.InvalidInput(s"missing canonical tool input field `$canonicalName`"))
            case Some(field) =>
              decode(field) match {
                case Left(message) => return Left(ToolInvokeError.InvalidInput(message))
                case Right(value)  => args += value
              }
          }
        case ToolMiddlewareParamDecoder.PrincipalParam => args += ctx.principal
        case ToolMiddlewareParamDecoder.StdinParam     =>
          ctx.stdin match {
            case Some(stream) => args += stream
            case None         =>
              return Left(ToolInvokeError.InvalidInput("tool invocation did not contain declared stdin stream"))
          }
      }
    }
    Right(args.result())
  }

  def fieldDecoder[A](
    from: FromSchema[A]
  ): CanonicalInputValue => Either[String, Any] =
    field => ToolInvokerRuntime.fieldDecoder(from)(field.value)

  def validateOutcome(
    tool: ExtendedToolType,
    commandIndex: Int,
    outcome: Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]
  ): Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult] =
    outcome match {
      case Right(result) =>
        validateSuccess(tool, commandIndex, result).map(_ => result)
      case Left(error @ ToolInvokeError.Tool(payload)) =>
        validateCustomError(tool, commandIndex, payload).fold(Left(_), _ => Left(error))
      case Left(protocol: ToolInvokeError.InvalidToolName)     => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidCommandPath)  => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidInput)        => Left(protocol)
      case Left(protocol: ToolInvokeError.ConstraintViolation) => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidResult)       => Left(protocol)
    }

  def validateRawInput(
    input: TypedSchemaValue
  ): Either[ToolInvokeError.InvalidInput, Unit] =
    validateSelfContainedValue(input, "tool invocation input").left
      .map(ToolInvokeError.InvalidInput.apply)

  def validateRawOutcome(
    outcome: Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]
  ): Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult] =
    outcome match {
      case Right(result) =>
        result.result match {
          case None        => Right(result)
          case Some(value) =>
            validateSelfContainedValue(value, "tool result")
              .fold(message => Left(ToolInvokeError.InvalidResult(message)), _ => Right(result))
        }
      case Left(error @ ToolInvokeError.Tool(payload)) =>
        validateSelfContainedValue(payload, "tool custom error")
          .fold(message => Left(ToolInvokeError.InvalidResult(message)), _ => Left(error))
      case Left(protocol: ToolInvokeError.InvalidToolName)     => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidCommandPath)  => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidInput)        => Left(protocol)
      case Left(protocol: ToolInvokeError.ConstraintViolation) => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidResult)       => Left(protocol)
    }

  private def validateSuccess(
    tool: ExtendedToolType,
    commandIndex: Int,
    result: ToolMiddlewareResult
  ): Either[ToolInvokeError.InvalidResult, Unit] =
    tool.commands.lift(commandIndex).flatMap(_.body) match {
      case None       => Left(ToolInvokeError.InvalidResult(s"invalid tool command index: $commandIndex"))
      case Some(body) =>
        val valueValidation = (body.result, result.result) match {
          case (None, None)                  => Right(())
          case (None, Some(_))               => Left("tool result unexpectedly contained a value")
          case (Some(_), None)               => Left("tool result did not contain a value")
          case (Some(expected), Some(value)) => validateTypedValue(value, expected.tpe, "tool result")
        }
        val stdoutValidation = (body.stdout.isDefined, result.stdout.isDefined) match {
          case (false, false) => Right(())
          case (false, true)  => Left("tool result unexpectedly contained stdout stream")
          case (true, false)  => Left("tool result did not contain declared stdout stream")
          case (true, true)   => Right(())
        }
        valueValidation
          .flatMap(_ => stdoutValidation)
          .left
          .map(ToolInvokeError.InvalidResult.apply)
    }

  private def validateCustomError(
    tool: ExtendedToolType,
    commandIndex: Int,
    value: TypedSchemaValue
  ): Either[ToolInvokeError.InvalidResult, Unit] =
    tool.commands.lift(commandIndex).flatMap(_.body) match {
      case None       => Left(ToolInvokeError.InvalidResult(s"invalid tool command index: $commandIndex"))
      case Some(body) =>
        val candidates = body.errors.map(_.payload.getOrElse(ToolErrorSupport.unitPayloadGraph))
        if (candidates.exists(expected => validateTypedValue(value, expected, "tool custom error").isRight)) Right(())
        else Left(ToolInvokeError.InvalidResult("tool custom error payload does not match a declared error case"))
    }

  private def validateTypedValue(
    value: TypedSchemaValue,
    expected: golem.schema.SchemaGraph,
    label: String
  ): Either[String, Unit] =
    WellFormedness.validateGraph(value.graph) match {
      case Left(errors)                                                     => Left(s"$label schema is invalid: ${errors.map(_.message).mkString("; ")}")
      case Right(_) if !ToolGraphs.schemaShapesMatch(value.graph, expected) =>
        Left(s"$label schema does not match the declared command result")
      case Right(_) =>
        ValueValidation
          .validateValue(expected, expected.root, value.value)
          .left
          .map(errors => s"$label value is invalid: ${errors.map(_.message).mkString("; ")}")
    }

  private def validateSelfContainedValue(
    value: TypedSchemaValue,
    label: String
  ): Either[String, Unit] =
    WellFormedness.validateGraph(value.graph) match {
      case Left(errors) => Left(s"$label schema is invalid: ${errors.map(_.message).mkString("; ")}")
      case Right(_)     =>
        ValueValidation
          .validateValue(value.graph, value.graph.root, value.value)
          .left
          .map(errors => s"$label value is invalid: ${errors.map(_.message).mkString("; ")}")
    }

  def encodeError[E](
    error: ToolInvokeError[E],
    schema: ToolErrorSchema[E]
  ): ToolInvokeError[TypedSchemaValue] =
    error match {
      case ToolInvokeError.Tool(value)                   => ToolInvokerRuntime.customError(value, schema)
      case protocol: ToolInvokeError.InvalidToolName     => protocol
      case protocol: ToolInvokeError.InvalidCommandPath  => protocol
      case protocol: ToolInvokeError.InvalidInput        => protocol
      case protocol: ToolInvokeError.ConstraintViolation => protocol
      case protocol: ToolInvokeError.InvalidResult       => protocol
    }

  def encodeInfallibleError(error: ToolInvokeError[Nothing]): ToolInvokeError[TypedSchemaValue] =
    error match {
      case protocol: ToolInvokeError.InvalidToolName     => protocol
      case protocol: ToolInvokeError.InvalidCommandPath  => protocol
      case protocol: ToolInvokeError.InvalidInput        => protocol
      case protocol: ToolInvokeError.ConstraintViolation => protocol
      case protocol: ToolInvokeError.InvalidResult       => protocol
    }

  def encodeUnit: Either[ToolInvokeError[Nothing], ToolMiddlewareResult] =
    Right(ToolMiddlewareResult(None, None))

  def encodeValue[A](
    value: A,
    into: golem.schema.IntoSchema[A]
  ): Either[ToolInvokeError[Nothing], ToolMiddlewareResult] =
    Right(ToolMiddlewareResult(Some(into.toTyped(value)), None))

  def encodeStdout(
    stdout: ToolMiddlewareOutputHandle,
    scoped: RawToolUnderlying
  ): Either[ToolInvokeError[Nothing], ToolMiddlewareResult] =
    Right(ToolMiddlewareResult(None, Some(ToolMiddlewareOwnershipRuntime.registerFinalStdout(scoped, stdout))))

  def encodeValueStdout[A](
    value: A,
    stdout: ToolMiddlewareOutputHandle,
    into: golem.schema.IntoSchema[A],
    scoped: RawToolUnderlying
  ): Either[ToolInvokeError[Nothing], ToolMiddlewareResult] = {
    val tracked = ToolMiddlewareOwnershipRuntime.registerFinalStdout(scoped, stdout)
    Right(ToolMiddlewareResult(Some(into.toTyped(value)), Some(tracked)))
  }

  private def failed[A](
    error: ToolInvokeError[TypedSchemaValue]
  ): Future[Either[ToolInvokeError[TypedSchemaValue], A]] =
    Future.successful(Left(error))
}

/** Runtime helpers used by generated nominal `<Tool>Underlying` projections. */
object ToolUnderlyingRuntime {
  private implicit val ec: scala.concurrent.ExecutionContext =
    ToolInvokerRuntime.executionContext

  def encodeParams(
    build: => List[(String, SchemaValue)]
  ): Either[ToolInvokeError[Nothing], List[(String, SchemaValue)]] =
    ToolClientRuntime.encodeParams(build).left.map(inputError)

  def countFlagValue(count: Int): SchemaValue =
    ToolClientRuntime.countFlagValue(count)

  def staticInputModel(
    descriptor: Either[ToolBuildError, ExtendedToolType],
    commandPath: List[String]
  ): Either[String, CanonicalInputModel] =
    ToolClientRuntime.staticInputModel(descriptor, commandPath)

  def buildInputFromModel(
    model: Either[String, CanonicalInputModel],
    paramValues: List[(String, SchemaValue)]
  ): Either[ToolInvokeError[Nothing], TypedSchemaValue] =
    ToolClientRuntime.buildInputFromModel(model, paramValues).left.map(inputError)

  def run[E](
    underlying: RawToolUnderlying,
    descriptor: Either[ToolBuildError, ExtendedToolType],
    commandPath: List[String],
    input: Either[ToolInvokeError[Nothing], TypedSchemaValue],
    stdin: Option[ToolMiddlewareInputHandle],
    decodeError: TypedSchemaValue => Either[String, E]
  ): Future[Either[ToolInvokeError[E], ToolMiddlewareResult]] =
    (descriptor, input) match {
      case (Left(error), _) =>
        Future.successful(Left(ToolInvokeError.InvalidResult(s"tool descriptor build failed: ${error.message}")))
      case (Right(_), Left(error))     => Future.successful(Left(error))
      case (Right(tool), Right(value)) =>
        val commandIndex = tool.commandIndexByPath(commandPath)
        if (commandIndex.isEmpty)
          return Future.successful(Left(ToolInvokeError.InvalidCommandPath(commandPath)))
        underlying
          .invoke(commandPath, value, stdin)
          .map { case outcome =>
            ToolMiddlewareInvokerRuntime.validateOutcome(tool, commandIndex.get, outcome)
          }
          .map {
            case Right(result)                     => Right(result)
            case Left(ToolInvokeError.Tool(value)) =>
              decodeError(value) match {
                case Right(error)  => Left(ToolInvokeError.Tool(error))
                case Left(message) => Left(ToolInvokeError.InvalidResult(message))
              }
            case Left(error: ToolInvokeError.InvalidToolName)     => Left(error)
            case Left(error: ToolInvokeError.InvalidCommandPath)  => Left(error)
            case Left(error: ToolInvokeError.InvalidInput)        => Left(error)
            case Left(error: ToolInvokeError.ConstraintViolation) => Left(error)
            case Left(error: ToolInvokeError.InvalidResult)       => Left(error)
          }
    }

  def runInfallible(
    underlying: RawToolUnderlying,
    descriptor: Either[ToolBuildError, ExtendedToolType],
    commandPath: List[String],
    input: Either[ToolInvokeError[Nothing], TypedSchemaValue],
    stdin: Option[ToolMiddlewareInputHandle]
  ): Future[Either[ToolInvokeError[Nothing], ToolMiddlewareResult]] =
    run[Nothing](
      underlying,
      descriptor,
      commandPath,
      input,
      stdin,
      _ => Left("an infallible tool returned a custom error")
    )

  def complete[E, T](
    call: Future[Either[ToolInvokeError[E], ToolMiddlewareResult]]
  )(decode: ToolMiddlewareResult => Either[ToolError[Nothing], T]): Future[Either[ToolInvokeError[E], T]] =
    call.map(_.flatMap(result => decode(result).left.map(resultError)))

  def decodeUnitResult(result: ToolMiddlewareResult): Either[ToolError[Nothing], Unit] =
    requireNoValue(result)

  def decodeValueResult[T](
    result: ToolMiddlewareResult,
    from: FromSchema[T]
  ): Either[ToolError[Nothing], T] =
    requireValue(result, from)

  def decodeStdoutResult(
    result: ToolMiddlewareResult
  ): Either[ToolError[Nothing], ToolMiddlewareOutputHandle] =
    for {
      stdout <- requireStdout(result)
      _      <- requireNoValue(result)
    } yield stdout

  def decodeValueStdoutResult[T](
    result: ToolMiddlewareResult,
    from: FromSchema[T]
  ): Either[ToolError[Nothing], (T, ToolMiddlewareOutputHandle)] =
    for {
      stdout <- requireStdout(result)
      value  <- requireValue(result, from)
    } yield (value, stdout)

  private def requireStdout(
    result: ToolMiddlewareResult
  ): Either[ToolError[Nothing], ToolMiddlewareOutputHandle] =
    result.stdout.toRight(resultError("tool result did not contain declared stdout stream"))

  private def requireValue[T](
    result: ToolMiddlewareResult,
    from: FromSchema[T]
  ): Either[ToolError[Nothing], T] =
    result.result match {
      case None        => Left(resultError("tool result did not contain a value"))
      case Some(value) => from.fromValue(value.value).left.map(error => resultError(error.message))
    }

  private def requireNoValue(result: ToolMiddlewareResult): Either[ToolError[Nothing], Unit] =
    if (result.result.isDefined)
      Left(resultError("tool result unexpectedly contained a value"))
    else Right(())

  private def inputError(error: ToolError[Nothing]): ToolInvokeError[Nothing] =
    ToolInvokeError.InvalidInput(toolErrorMessage(error))

  private def resultError(error: ToolError[Nothing]): ToolInvokeError[Nothing] =
    ToolInvokeError.InvalidResult(toolErrorMessage(error))

  private def resultError(message: String): ToolError[Nothing] =
    ToolError.Rpc(RpcError.Protocol(message))

  private def toolErrorMessage(error: ToolError[Nothing]): String =
    error match {
      case ToolError.Rpc(rpc) => rpc.message
      case ToolError.Tool(_)  => "unexpected typed tool error"
    }
}

trait UniversalToolUnderlying extends RawToolUnderlying

final case class UniversalToolMiddlewareInvocation(
  toolName: String,
  toolMetadata: WitTool,
  commandPath: List[String],
  input: TypedSchemaValue,
  stdin: Option[ToolMiddlewareInputHandle],
  principal: Principal
)

trait UniversalToolMiddleware {
  def invoke(
    invocation: UniversalToolMiddlewareInvocation,
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]]
}

object UniversalToolMiddlewareInvokerRuntime {
  private implicit val ec: scala.concurrent.ExecutionContext =
    ToolInvokerRuntime.executionContext

  def invoke(
    handle: UniversalToolMiddlewareHandle,
    wrapped: RawToolUnderlying,
    toolName: String,
    toolMetadata: WitTool,
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolMiddlewareInputHandle],
    principal: Principal,
    validateFinalStdout: ToolMiddlewareOutputHandle => Either[String, Unit] = _ => Right(())
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
    ToolMiddlewareOwnershipRuntime.withInvocationScopedUnderlying(wrapped, stdin, validateFinalStdout) { scoped =>
      val instance = handle.newInstance()
      ToolMiddlewareInvokerRuntime.validateRawInput(input) match {
        case Left(error) => Future.successful(Left(error))
        case Right(_)    =>
          val underlying = new UniversalToolUnderlying {
            def invoke(
              commandPath: List[String],
              input: TypedSchemaValue,
              stdin: Option[ToolMiddlewareInputHandle]
            ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
              scoped.invoke(commandPath, input, stdin)
          }
          instance
            .invoke(
              UniversalToolMiddlewareInvocation(
                toolName,
                toolMetadata,
                commandPath,
                input,
                stdin,
                principal
              ),
              underlying
            )
            .map(outcome =>
              ToolMiddlewareOwnershipRuntime.validateFinal(scoped, outcome)(
                ToolMiddlewareInvokerRuntime.validateRawOutcome
              )
            )
      }
    }
}

private[golem] object ToolMiddlewareOwnershipRuntime {
  private implicit val ec: scala.concurrent.ExecutionContext =
    ToolInvokerRuntime.executionContext

  def withInvocationScopedUnderlying(
    raw: RawToolUnderlying,
    stdin: Option[ToolMiddlewareInputHandle],
    validateFinalStdout: ToolMiddlewareOutputHandle => Either[String, Unit] = _ => Right(())
  )(
    invoke: RawToolUnderlying => Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]]
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] = {
    val ownership = new InvocationOwnership(stdin)
    val scoped    = new InvocationScopedUnderlying(raw, ownership)
    val outcome   =
      try invoke(scoped)
      catch {
        case error: Throwable => Future.failed(error)
      }

    outcome.transformWith { completed =>
      val result = completed.flatMap {
        case Right(value) =>
          Try(ownership.validateStdout(value.stdout, validateFinalStdout)).flatMap {
            case Left(message) => Success(Left(ToolInvokeError.InvalidResult(message)))
            case Right(_)      => Try(value.copy(stdout = ownership.releaseStdout(value.stdout))).map(Right(_))
          }
        case failure => Success(failure)
      }
      cleanup(scoped.revoke())
        .flatMap(_ => cleanup(ownership.dispose()))
        .flatMap(_ => Future.fromTry(result))
    }
  }

  def validateFinal(
    scoped: RawToolUnderlying,
    outcome: Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]
  )(
    validate: Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult] => Either[
      ToolInvokeError[TypedSchemaValue],
      ToolMiddlewareResult
    ]
  ): Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult] =
    scoped match {
      case invocation: InvocationScopedUnderlying => validate(invocation.trackFinal(outcome))
      case _                                      => validate(outcome)
    }

  def registerFinalStdout(
    scoped: RawToolUnderlying,
    stream: ToolMiddlewareOutputHandle
  ): ToolMiddlewareOutputHandle =
    scoped match {
      case invocation: InvocationScopedUnderlying => invocation.trackFinalStdout(stream)
      case _                                      => stream
    }

  private final class InvocationScopedUnderlying(
    raw: RawToolUnderlying,
    ownership: InvocationOwnership
  ) extends RawToolUnderlying {
    private type Outcome = Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]

    private var revoked                                    = false
    private var inFlight                                   = false
    private var activeInvocation: Option[Promise[Outcome]] = None

    def invoke(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolMiddlewareInputHandle]
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] = {
      val completion = synchronized {
        if (revoked)
          return Future.failed(new ToolUnderlyingMisuseException(ToolUnderlyingMisuse.Revoked))
        if (inFlight)
          return Future.failed(new ToolUnderlyingMisuseException(ToolUnderlyingMisuse.OverlappingInvocation))
        inFlight = true
        val result = Promise[Outcome]()
        activeInvocation = Some(result)
        result
      }

      val invocation =
        ToolMiddlewareInvokerRuntime.validateRawInput(input) match {
          case Left(error) => Future.successful(Left(error))
          case Right(_)    =>
            try {
              ownership.forwardStdin(stdin)
              raw.invoke(commandPath, input, stdin).map(ownership.trackAndValidate)
            } catch {
              case error: Throwable => Future.failed(error)
            }
        }
      invocation.onComplete { result =>
        synchronized {
          completion.tryComplete(result)
          if (activeInvocation.exists(_ eq completion)) {
            activeInvocation = None
            inFlight = false
          }
        }
      }
      completion.future
    }

    def revoke(): Future[Unit] = {
      val active = synchronized {
        revoked = true
        activeInvocation
      }
      active
        .map(_.future.map(_ => ()).recover { case _ => () })
        .getOrElse(Future.successful(()))
    }

    def trackFinal(
      outcome: Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]
    ): Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult] =
      ownership.track(outcome)

    def trackFinalStdout(stream: ToolMiddlewareOutputHandle): ToolMiddlewareOutputHandle =
      ownership.trackStdout(stream)
  }

  private final class InvocationOwnership(outerStdin: Option[ToolMiddlewareInputHandle]) {
    private val transferred           = mutable.ListBuffer.empty[AnyRef]
    private val stdout                = mutable.ListBuffer.empty[(ToolMiddlewareOutputHandle, TrackedOutputHandle)]
    private var outerStdinTransferred = false

    def forwardStdin(stream: Option[ToolMiddlewareInputHandle]): Unit =
      stream.foreach(transfer)

    def trackAndValidate(
      outcome: Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]
    ): Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult] =
      ToolMiddlewareInvokerRuntime.validateRawOutcome(track(outcome))

    def track(
      outcome: Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]
    ): Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult] =
      outcome.map(result => result.copy(stdout = result.stdout.map(trackStdout)))

    def validateStdout(
      stream: Option[ToolMiddlewareOutputHandle],
      validate: ToolMiddlewareOutputHandle => Either[String, Unit]
    ): Either[String, Unit] =
      stream match {
        case Some(value) => validate(trackStdout(value).release())
        case None        => Right(())
      }

    def releaseStdout(
      stream: Option[ToolMiddlewareOutputHandle]
    ): Option[ToolMiddlewareOutputHandle] =
      stream.map { value =>
        val tracked = trackStdout(value)
        transfer(tracked)
        tracked.release()
      }

    def dispose(): Future[Unit] = {
      val actions = synchronized {
        val inputs = outerStdin.filterNot(_ => outerStdinTransferred).toList.map(stream => () => stream.close())
        inputs ++ stdout.toList.map { case (_, stream) => () => stream.disposeIfOwned() }
      }
      Future.sequence(actions.map(action => cleanup(action()))).map(_ => ())
    }

    def trackStdout(stream: ToolMiddlewareOutputHandle): TrackedOutputHandle =
      synchronized {
        stdout.collectFirst {
          case (raw, tracked) if (raw eq stream) || (tracked eq stream) => tracked
        }.getOrElse {
          val tracked = new TrackedOutputHandle(stream)
          stdout += ((stream, tracked))
          tracked
        }
      }

    private def transfer(stream: AnyRef): Unit =
      synchronized {
        if (transferred.exists(_ eq stream))
          throw new ToolUnderlyingMisuseException(ToolUnderlyingMisuse.StreamAlreadyTransferred)
        transferred += stream
        if (outerStdin.exists(_ eq stream)) outerStdinTransferred = true
        stdout.collectFirst {
          case (raw, tracked) if (raw eq stream) || (tracked eq stream) => tracked
        }.foreach(_.transfer())
      }
  }

  private final class TrackedOutputHandle(underlying: ToolMiddlewareOutputHandle) extends ToolMiddlewareOutputHandle {
    private var transferred = false
    private var closed      = false

    def transfer(): Unit = synchronized {
      transferred = true
    }

    def release(): ToolMiddlewareOutputHandle = underlying

    def disposeIfOwned(): Future[Unit] = synchronized {
      if (transferred) Future.successful(())
      else close()
    }

    override private[golem] def close(): Future[Unit] = synchronized {
      if (closed) Future.successful(())
      else {
        closed = true
        underlying.close()
      }
    }
  }

  private def cleanup(action: => Future[Unit]): Future[Unit] =
    try action.recover { case _ => () }
    catch {
      case _: Throwable => Future.successful(())
    }
}

sealed trait ToolUnderlyingMisuse extends Product with Serializable {
  def message: String
}

object ToolUnderlyingMisuse {
  case object OverlappingInvocation extends ToolUnderlyingMisuse {
    val message: String = "an underlying tool invocation is already in flight"
  }

  case object Revoked extends ToolUnderlyingMisuse {
    val message: String = "the underlying tool is no longer available"
  }

  case object StreamAlreadyTransferred extends ToolUnderlyingMisuse {
    val message: String = "the stream was already transferred"
  }
}

final class ToolUnderlyingMisuseException(val reason: ToolUnderlyingMisuse)
    extends IllegalStateException(reason.message)
