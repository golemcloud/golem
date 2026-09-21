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
import golem.schema.{FromSchema, SchemaGraph, SchemaType, SchemaTypeBody, SchemaValue, TypedSchemaValue}
import golem.schema.validation.{ValueValidation, WellFormedness}
import golem.tool.wire.WitTool

import scala.collection.mutable
import scala.concurrent.{Future, Promise}
import scala.util.{Success, Try}

final case class ToolMiddlewareDescriptor(
  name: String,
  aliases: List[String],
  doc: Doc,
  scope: ToolMiddlewareScope,
  parameterSchema: SchemaGraph,
  version: String = "0.0.0"
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

  case object PrincipalParam                                     extends ToolMiddlewareParamDecoder
  case object StdinParam                                         extends ToolMiddlewareParamDecoder
  final case class InstallationParameters(from: FromSchema[Any]) extends ToolMiddlewareParamDecoder
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
  decodeParameters: TypedSchemaValue => Either[String, Any],
  newInstance: () => UniversalToolMiddleware.Internal
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

  def start(
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolMiddlewareInputHandle]
  ): ToolUnderlyingInvocation[TypedSchemaValue, ToolMiddlewareResult] = {
    val terminal = invoke(commandPath, input, stdin)
      .map(_.left.map(ToolUnderlyingError.Tool(_)))(ToolInvokerRuntime.executionContext)
    ToolUnderlyingInvocation(
      Future.successful(
        ToolUnderlyingAdmission(
          None,
          () => terminal,
          () => (),
          () => ()
        )
      )
    )
  }
}

sealed trait ToolUnderlyingError[+E] extends Product with Serializable
object ToolUnderlyingError {
  final case class Tool[E](error: ToolInvokeError[E]) extends ToolUnderlyingError[E]
  final case class ProtocolError(message: String)     extends ToolUnderlyingError[Nothing]
  final case class Denied(message: String)            extends ToolUnderlyingError[Nothing]
  final case class InternalError(message: String)     extends ToolUnderlyingError[Nothing]
  case object Cancelled                               extends ToolUnderlyingError[Nothing]
  final case class ResourceExhausted(message: String) extends ToolUnderlyingError[Nothing]
}

/**
 * One independently observable invocation of the next middleware-chain layer.
 */
final case class ToolUnderlyingInvocation[+E, +A](
  admission: Future[ToolUnderlyingAdmission[E, A]]
) {
  def toMiddlewareResult: Future[Either[ToolInvokeError[E], A]] =
    admission
      .flatMap(_.result)(ToolInvokerRuntime.executionContext)
      .map {
        case Right(value)                                     => Right(value)
        case Left(ToolUnderlyingError.Tool(error))            => Left(error)
        case Left(ToolUnderlyingError.ProtocolError(message)) => Left(ToolInvokeError.ProtocolError(message))
        case Left(ToolUnderlyingError.Denied(message))        => Left(ToolInvokeError.Denied(message))
        case Left(ToolUnderlyingError.InternalError(message)) => Left(ToolInvokeError.InternalError(message))
        case Left(ToolUnderlyingError.Cancelled)              =>
          Left(ToolInvokeError.Cancelled)
        case Left(ToolUnderlyingError.ResourceExhausted(message)) =>
          Left(ToolInvokeError.ResourceExhausted(message))
      }(ToolInvokerRuntime.executionContext)
}

final case class ToolUnderlyingAdmission[+E, +A](
  stdout: Option[ToolMiddlewareOutputHandle],
  startResult: () => Future[Either[ToolUnderlyingError[E], A]],
  cancel: () => Unit,
  drop: () => Unit
) {
  lazy val result: Future[Either[ToolUnderlyingError[E], A]] = startResult()
}

final class ToolMiddlewareInvocationContext(
  val fields: List[CanonicalInputValue],
  val parameters: TypedSchemaValue,
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
    parameters: TypedSchemaValue,
    stdin: Option[ToolMiddlewareInputHandle],
    principal: Principal,
    validateFinalStdout: ToolMiddlewareOutputHandle => Either[String, Unit] = _ => Right(())
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
    ToolMiddlewareOwnershipRuntime.withInvocationScopedUnderlying(wrapped, stdin, validateFinalStdout) { underlying =>
      handle.descriptor(new ToolBuildCtx) match {
        case Left(error) =>
          failed(ToolInvokeError.InvalidInput(s"tool middleware descriptor build failed: ${error.message}"))
        case Right(descriptor) if !ToolGraphs.schemaShapesMatch(parameters.graph, descriptor.parameterSchema) =>
          failed(
            ToolInvokeError.InvalidInput("middleware installation parameter schema does not match its declaration")
          )
        case Right(_)
            if ValueValidation.validateValue(parameters.graph, parameters.graph.root, parameters.value).isLeft =>
          failed(ToolInvokeError.InvalidInput("middleware installation parameter value does not match its schema"))
        case Right(_) if toolName != presented.toolName =>
          failed(ToolInvokeError.InvalidToolName(toolName))
        case Right(_) =>
          val instance = handle.newInstance()
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
                        .find(binding =>
                          presented.commandIndexByPath(binding.commandPath).contains(commandIndex)
                        ) match {
                        case None                                                     => failed(ToolInvokeError.InvalidCommandPath(commandPath))
                        case Some(binding) if stdin.nonEmpty && !binding.expectsStdin =>
                          failed(ToolInvokeError.InvalidInput("tool invocation contained unexpected stdin stream"))
                        case Some(binding) =>
                          binding
                            .run(
                              instance,
                              underlying,
                              new ToolMiddlewareInvocationContext(fields, parameters, stdin, principal)
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
        case ToolMiddlewareParamDecoder.InstallationParameters(from) =>
          from.fromValue(ctx.parameters.value) match {
            case Left(error)  => return Left(ToolInvokeError.InvalidInput(error.message))
            case Right(value) => args += value
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
      case Left(error @ ToolInvokeError.UnknownToolError(name, payload)) =>
        validateCustomError(tool, commandIndex, name, payload).fold(Left(_), _ => Left(error))
      case Left(protocol: ToolInvokeError.InvalidToolName)     => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidCommandPath)  => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidInput)        => Left(protocol)
      case Left(protocol: ToolInvokeError.ConstraintViolation) => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidResult)       => Left(protocol)
      case Left(protocol: ToolInvokeError.ProtocolError)       => Left(protocol)
      case Left(protocol: ToolInvokeError.Denied)              => Left(protocol)
      case Left(protocol: ToolInvokeError.InternalError)       => Left(protocol)
      case Left(ToolInvokeError.Cancelled)                     => Left(ToolInvokeError.Cancelled)
      case Left(protocol: ToolInvokeError.ResourceExhausted)   => Left(protocol)
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
      case Left(error @ ToolInvokeError.UnknownToolError(_, payload)) =>
        validateSelfContainedValue(payload, "tool custom error")
          .fold(message => Left(ToolInvokeError.InvalidResult(message)), _ => Left(error))
      case Left(protocol: ToolInvokeError.InvalidToolName)     => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidCommandPath)  => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidInput)        => Left(protocol)
      case Left(protocol: ToolInvokeError.ConstraintViolation) => Left(protocol)
      case Left(protocol: ToolInvokeError.InvalidResult)       => Left(protocol)
      case Left(protocol: ToolInvokeError.ProtocolError)       => Left(protocol)
      case Left(protocol: ToolInvokeError.Denied)              => Left(protocol)
      case Left(protocol: ToolInvokeError.InternalError)       => Left(protocol)
      case Left(ToolInvokeError.Cancelled)                     => Left(ToolInvokeError.Cancelled)
      case Left(protocol: ToolInvokeError.ResourceExhausted)   => Left(protocol)
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
    validateCustomErrorByPayload(tool, commandIndex, None, value)

  private def validateCustomError(
    tool: ExtendedToolType,
    commandIndex: Int,
    name: String,
    value: TypedSchemaValue
  ): Either[ToolInvokeError.InvalidResult, Unit] =
    validateCustomErrorByPayload(tool, commandIndex, Some(name), value)

  private def validateCustomErrorByPayload(
    tool: ExtendedToolType,
    commandIndex: Int,
    name: Option[String],
    value: TypedSchemaValue
  ): Either[ToolInvokeError.InvalidResult, Unit] =
    tool.commands.lift(commandIndex).flatMap(_.body) match {
      case None       => Left(ToolInvokeError.InvalidResult(s"invalid tool command index: $commandIndex"))
      case Some(body) =>
        val candidates = body.errors
          .filter(error => name.forall(_ == error.name))
          .map(_.payload.getOrElse(ToolErrorSupport.unitPayloadGraph))
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
      case unknown: ToolInvokeError.UnknownToolError     => unknown
      case protocol: ToolInvokeError.InvalidToolName     => protocol
      case protocol: ToolInvokeError.InvalidCommandPath  => protocol
      case protocol: ToolInvokeError.InvalidInput        => protocol
      case protocol: ToolInvokeError.ConstraintViolation => protocol
      case protocol: ToolInvokeError.InvalidResult       => protocol
      case protocol: ToolInvokeError.ProtocolError       => protocol
      case protocol: ToolInvokeError.Denied              => protocol
      case protocol: ToolInvokeError.InternalError       => protocol
      case ToolInvokeError.Cancelled                     => ToolInvokeError.Cancelled
      case protocol: ToolInvokeError.ResourceExhausted   => protocol
    }

  def encodeInfallibleError(error: ToolInvokeError[Nothing]): ToolInvokeError[TypedSchemaValue] =
    error match {
      case unknown: ToolInvokeError.UnknownToolError     => unknown
      case protocol: ToolInvokeError.InvalidToolName     => protocol
      case protocol: ToolInvokeError.InvalidCommandPath  => protocol
      case protocol: ToolInvokeError.InvalidInput        => protocol
      case protocol: ToolInvokeError.ConstraintViolation => protocol
      case protocol: ToolInvokeError.InvalidResult       => protocol
      case protocol: ToolInvokeError.ProtocolError       => protocol
      case protocol: ToolInvokeError.Denied              => protocol
      case protocol: ToolInvokeError.InternalError       => protocol
      case ToolInvokeError.Cancelled                     => ToolInvokeError.Cancelled
      case protocol: ToolInvokeError.ResourceExhausted   => protocol
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
    decodeError: NamedToolError => Either[String, E]
  ): ToolUnderlyingInvocation[E, ToolMiddlewareResult] =
    (descriptor, input) match {
      case (Left(error), _) =>
        completed(Left(ToolInvokeError.InvalidResult(s"tool descriptor build failed: ${error.message}")))
      case (Right(_), Left(error))     => completed(Left(error))
      case (Right(tool), Right(value)) =>
        val commandIndex = tool.commandIndexByPath(commandPath)
        if (commandIndex.isEmpty)
          return completed(Left(ToolInvokeError.InvalidCommandPath(commandPath)))
        val started = underlying.start(commandPath, value, stdin)
        ToolUnderlyingInvocation(started.admission.map { admission =>
          admission.copy(startResult =
            () =>
              admission.result.map {
                case Left(ToolUnderlyingError.Tool(error)) =>
                  Left(ToolUnderlyingError.Tool(mapDeclared(error, decodeError)))
                case Left(error: ToolUnderlyingError.ProtocolError)     => Left(error)
                case Left(error: ToolUnderlyingError.Denied)            => Left(error)
                case Left(error: ToolUnderlyingError.InternalError)     => Left(error)
                case Left(ToolUnderlyingError.Cancelled)                => Left(ToolUnderlyingError.Cancelled)
                case Left(error: ToolUnderlyingError.ResourceExhausted) => Left(error)
                case Right(result)                                      =>
                  ToolMiddlewareInvokerRuntime.validateOutcome(tool, commandIndex.get, Right(result)) match {
                    case Right(valid) => Right(valid)
                    case Left(error)  => Left(ToolUnderlyingError.Tool(mapDeclared(error, decodeError)))
                  }
              }
          )
        })
    }

  private def mapDeclared[E](
    error: ToolInvokeError[TypedSchemaValue],
    decodeError: NamedToolError => Either[String, E]
  ): ToolInvokeError[E] =
    error match {
      case ToolInvokeError.UnknownToolError(name, payload) =>
        decodeError(NamedToolError(name, payload)) match {
          case Right(value) => ToolInvokeError.Tool(value)
          case Left(_)      => ToolInvokeError.UnknownToolError(name, payload)
        }
      case ToolInvokeError.Tool(_) =>
        ToolInvokeError.InvalidResult("underlying custom error was missing its declared case name")
      case other => other.asInstanceOf[ToolInvokeError[E]]
    }

  private def completed[E](
    value: Either[ToolInvokeError[E], ToolMiddlewareResult]
  ): ToolUnderlyingInvocation[E, ToolMiddlewareResult] =
    ToolUnderlyingInvocation(
      Future.successful(
        ToolUnderlyingAdmission(
          None,
          () => Future.successful(value.left.map(ToolUnderlyingError.Tool(_))),
          () => (),
          () => ()
        )
      )
    )

  def runInfallible(
    underlying: RawToolUnderlying,
    descriptor: Either[ToolBuildError, ExtendedToolType],
    commandPath: List[String],
    input: Either[ToolInvokeError[Nothing], TypedSchemaValue],
    stdin: Option[ToolMiddlewareInputHandle]
  ): ToolUnderlyingInvocation[Nothing, ToolMiddlewareResult] =
    run[Nothing](
      underlying,
      descriptor,
      commandPath,
      input,
      stdin,
      _ => Left("an infallible tool returned a custom error")
    )

  def complete[E, T](call: ToolUnderlyingInvocation[E, ToolMiddlewareResult])(
    decode: ToolMiddlewareResult => Either[ToolError[Nothing], T]
  ): ToolUnderlyingInvocation[E, T] =
    ToolUnderlyingInvocation(
      call.admission.map(admission =>
        admission.copy(startResult =
          () =>
            admission.result.map(
              _.flatMap(result => decode(result).left.map(error => ToolUnderlyingError.Tool(resultError(error))))
            )
        )
      )
    )

  def decodeUnitResult(result: ToolMiddlewareResult): Either[ToolError[Nothing], Unit] =
    requireNoValue(result)

  def decodeValueResult[T](
    result: ToolMiddlewareResult,
    from: FromSchema[T]
  ): Either[ToolError[Nothing], T] =
    requireValue(result, from)

  def decodeValueResult[T](
    result: ToolMiddlewareResult,
    from: FromSchema[T],
    expected: golem.schema.SchemaGraph
  ): Either[ToolError[Nothing], T] =
    requireValue(result, from, Some(expected))

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

  def decodeValueStdoutResult[T](
    result: ToolMiddlewareResult,
    from: FromSchema[T],
    expected: golem.schema.SchemaGraph
  ): Either[ToolError[Nothing], (T, ToolMiddlewareOutputHandle)] =
    for {
      stdout <- requireStdout(result)
      value  <- requireValue(result, from, Some(expected))
    } yield (value, stdout)

  private def requireStdout(
    result: ToolMiddlewareResult
  ): Either[ToolError[Nothing], ToolMiddlewareOutputHandle] =
    result.stdout.toRight(resultError("tool result did not contain declared stdout stream"))

  private def requireValue[T](
    result: ToolMiddlewareResult,
    from: FromSchema[T],
    expected: Option[golem.schema.SchemaGraph] = None
  ): Either[ToolError[Nothing], T] =
    result.result match {
      case None                                                                                       => Left(resultError("tool result did not contain a value"))
      case Some(value) if expected.exists(graph => !ToolGraphs.schemaShapesMatch(value.graph, graph)) =>
        Left(resultError("tool result schema does not match the generated underlying client's expected result schema"))
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
      case ToolError.Rpc(rpc)                       => rpc.message
      case ToolError.RemoteTool(remote)             => s"remote tool error: $remote"
      case ToolError.Tool(_)                        => "unexpected typed tool error"
      case ToolError.UnknownToolError(name, _)      => s"unexpected tool error `$name`"
      case ToolError.InvalidInput(message)          => message
      case ToolError.MalformedRemoteOutput(message) => message
    }
}

trait UniversalToolUnderlying extends RawToolUnderlying

final case class UniversalToolMiddlewareInvocation[Parameters](
  toolName: String,
  toolMetadata: WitTool,
  parameters: Parameters,
  commandPath: List[String],
  input: TypedSchemaValue,
  stdin: Option[ToolMiddlewareInputHandle],
  principal: Principal
)

trait UniversalToolMiddleware extends UniversalToolMiddleware.Internal {
  def invoke(
    invocation: UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters],
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]]

  final private[golem] def invokeAny(
    invocation: UniversalToolMiddlewareInvocation[Any],
    underlying: UniversalToolUnderlying
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
    invoke(invocation.asInstanceOf[UniversalToolMiddlewareInvocation[ToolMiddleware.NoParameters]], underlying)
}

object UniversalToolMiddleware {
  private[golem] trait Internal {
    private[golem] def invokeAny(
      invocation: UniversalToolMiddlewareInvocation[Any],
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]]
  }

  trait WithParameters[Parameters] extends Internal {
    def invoke(
      invocation: UniversalToolMiddlewareInvocation[Parameters],
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]]

    final private[golem] def invokeAny(
      invocation: UniversalToolMiddlewareInvocation[Any],
      underlying: UniversalToolUnderlying
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
      invoke(invocation.asInstanceOf[UniversalToolMiddlewareInvocation[Parameters]], underlying)
  }
}

object UniversalToolMiddlewareInvokerRuntime {
  private implicit val ec: scala.concurrent.ExecutionContext =
    ToolInvokerRuntime.executionContext

  def invoke(
    handle: UniversalToolMiddlewareHandle,
    wrapped: RawToolUnderlying,
    toolName: String,
    toolMetadata: WitTool,
    parameters: TypedSchemaValue,
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolMiddlewareInputHandle],
    principal: Principal,
    validateFinalStdout: ToolMiddlewareOutputHandle => Either[String, Unit] = _ => Right(())
  ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
    ToolMiddlewareOwnershipRuntime.withInvocationScopedUnderlying(wrapped, stdin, validateFinalStdout) { scoped =>
      val instance = handle.newInstance()
      ToolMiddlewareInvokerRuntime.validateRawInput(input) match {
        case Left(error)                                                                                    => Future.successful(Left(error))
        case Right(_) if !ToolGraphs.schemaShapesMatch(parameters.graph, handle.descriptor.parameterSchema) =>
          Future.successful(
            Left(
              ToolInvokeError.InvalidInput("middleware installation parameter schema does not match its declaration")
            )
          )
        case Right(_) =>
          handle.decodeParameters(parameters) match {
            case Left(error)              => Future.successful(Left(ToolInvokeError.InvalidInput(error)))
            case Right(decodedParameters) =>
              val underlying = new UniversalToolUnderlying {
                def invoke(
                  commandPath: List[String],
                  input: TypedSchemaValue,
                  stdin: Option[ToolMiddlewareInputHandle]
                ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
                  scoped.invoke(commandPath, input, stdin)

                override def start(
                  commandPath: List[String],
                  input: TypedSchemaValue,
                  stdin: Option[ToolMiddlewareInputHandle]
                ): ToolUnderlyingInvocation[TypedSchemaValue, ToolMiddlewareResult] =
                  scoped.start(commandPath, input, stdin)
              }
              instance
                .invokeAny(
                  UniversalToolMiddlewareInvocation(
                    toolName,
                    toolMetadata,
                    decodedParameters,
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
}

object ToolMiddleware {
  final case class NoParameters()

  val noParametersSchema: SchemaGraph =
    SchemaGraph(scala.collection.immutable.ListMap.empty, SchemaType(SchemaTypeBody.RecordType(Nil)))

  val noParametersValue: TypedSchemaValue =
    TypedSchemaValue(noParametersSchema, SchemaValue.RecordValue(Nil))
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
      scoped.revokeAdmission()
      cleanup(scoped.drain())
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
    private type Admission = ToolUnderlyingAdmission[TypedSchemaValue, ToolMiddlewareResult]

    private var revoked           = false
    private val activeInvocations = mutable.ListBuffer.empty[Promise[Admission]]

    def invoke(
      commandPath: List[String],
      input: TypedSchemaValue,
      stdin: Option[ToolMiddlewareInputHandle]
    ): Future[Either[ToolInvokeError[TypedSchemaValue], ToolMiddlewareResult]] =
      start(commandPath, input, stdin).admission.flatMap { admission =>
        admission.result.map {
          case Right(value)                                     => Right(value)
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
      input: TypedSchemaValue,
      stdin: Option[ToolMiddlewareInputHandle]
    ): ToolUnderlyingInvocation[TypedSchemaValue, ToolMiddlewareResult] = {
      val completion = {
        if (revoked)
          return ToolUnderlyingInvocation(
            Future.failed(new ToolUnderlyingMisuseException(ToolUnderlyingMisuse.Revoked))
          )
        val result = Promise[Admission]()
        activeInvocations += result
        result
      }

      val invocation: Future[Admission] =
        ToolMiddlewareInvokerRuntime.validateRawInput(input) match {
          case Left(error) =>
            Future.successful(
              ToolUnderlyingAdmission(
                None,
                () => Future.successful(Left(ToolUnderlyingError.Tool(error))),
                () => (),
                () => ()
              )
            )
          case Right(_) =>
            try {
              ownership.forwardStdin(stdin)
              val started = raw.start(commandPath, input, stdin)
              started.admission.map { admission =>
                val trackedStdout = admission.stdout.map(ownership.trackStdout)
                admission.copy(
                  stdout = trackedStdout,
                  startResult = () =>
                    admission.result.map {
                      case Right(value) =>
                        val merged = value.copy(stdout = trackedStdout.orElse(value.stdout))
                        ownership.trackAndValidate(Right(merged)).left.map(ToolUnderlyingError.Tool(_))
                      case Left(error) => Left(error)
                    }
                )
              }
            } catch {
              case error: Throwable => Future.failed(error)
            }
        }
      invocation.onComplete { result =>
        completion.tryComplete(result)
      }
      ToolUnderlyingInvocation(completion.future)
    }

    def revokeAdmission(): Unit = revoked = true

    def drain(): Future[Unit] =
      Future
        .sequence(activeInvocations.toList.map(_.future.map { admission =>
          admission.drop()
        }.recover { case _ => () }))
        .map(_ => ())

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
      val actions = {
        val inputs = outerStdin.filterNot(_ => outerStdinTransferred).toList.map(stream => () => stream.close())
        inputs ++ stdout.toList.map { case (_, stream) => () => stream.disposeIfOwned() }
      }
      Future.sequence(actions.map(action => cleanup(action()))).map(_ => ())
    }

    def trackStdout(stream: ToolMiddlewareOutputHandle): TrackedOutputHandle =
      stdout.collectFirst {
        case (raw, tracked) if (raw eq stream) || (tracked eq stream) => tracked
      }.getOrElse {
        val tracked = new TrackedOutputHandle(stream)
        stdout += ((stream, tracked))
        tracked
      }

    private def transfer(stream: AnyRef): Unit = {
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

    def transfer(): Unit =
      transferred = true

    def release(): ToolMiddlewareOutputHandle = underlying

    def disposeIfOwned(): Future[Unit] =
      if (transferred) Future.successful(())
      else close()

    override private[golem] def close(): Future[Unit] =
      if (closed) Future.successful(())
      else {
        closed = true
        underlying.close()
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
  case object Revoked extends ToolUnderlyingMisuse {
    val message: String = "the underlying tool is no longer available"
  }

  case object StreamAlreadyTransferred extends ToolUnderlyingMisuse {
    val message: String = "the stream was already transferred"
  }
}

final class ToolUnderlyingMisuseException(val reason: ToolUnderlyingMisuse)
    extends IllegalStateException(reason.message)
