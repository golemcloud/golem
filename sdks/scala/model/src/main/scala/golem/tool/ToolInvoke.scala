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
import golem.schema.{FromSchema, IntoSchema, SchemaEncodeError, SchemaValue, TypedSchemaValue}

import scala.concurrent.{ExecutionContext, Future}

/**
 * Platform-neutral mirror of the wire `tool-error`, used by macro-generated
 * tool invokers. The platform layer converts it to the wire representation at
 * the guest-export boundary.
 */
sealed trait ToolInvokeError extends Product with Serializable
object ToolInvokeError {
  final case class InvalidToolName(name: String)          extends ToolInvokeError
  final case class InvalidCommandPath(path: List[String]) extends ToolInvokeError
  final case class InvalidInput(message: String)          extends ToolInvokeError
  final case class ConstraintViolation(message: String)   extends ToolInvokeError
  final case class InvalidResult(message: String)         extends ToolInvokeError
  final case class Custom(payload: TypedSchemaValue)      extends ToolInvokeError
}

/**
 * A successful tool invocation outcome: the optional structured result and the
 * optional stdout stream handle.
 */
final case class ToolInvokeResult(
  result: Option[TypedSchemaValue],
  stdout: Option[ToolOutputStream]
)

/** A registered tool's platform-neutral invocation entry point. */
trait ToolInvokeHandler {
  def invoke(
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream],
    principal: Principal
  ): Future[Either[ToolInvokeError, ToolInvokeResult]]
}

/**
 * The environment a tool invoker runs in: stdout acquisition and lookup of
 * sibling tools for subtree forwarding. The platform layer supplies the real
 * implementation (backed by the tool registry); tests may use fakes.
 */
trait ToolInvokeEnv {
  def stdout(): ToolOutputStream
  def invokerFor(toolName: String): Option[ToolInvokeHandler]
  def extendedToolFor(toolName: String): Option[ExtendedToolType]
}

/** The per-invocation context handed to a method binding's run function. */
final class ToolInvocationContext(
  val fields: List[CanonicalInputValue],
  val stdin: Option[ToolInputStream],
  val principal: Principal,
  val env: ToolInvokeEnv
)

/**
 * One tool method's dispatch entry: the command path it serves and the
 * macro-generated run function that decodes its parameters, calls the
 * implementation, and encodes the outcome.
 */
final case class ToolMethodBinding(
  methodName: String,
  commandPath: List[String],
  run: ToolInvocationContext => Future[Either[ToolInvokeError, ToolInvokeResult]]
)

/**
 * A subtree link: invocations whose command path starts with `pathPrefix` are
 * forwarded to the separately registered child tool `childToolName`.
 */
final case class ToolSubtreeForward(
  pathPrefix: List[String],
  childToolName: String
)

/**
 * Everything the tool-implementation macro produces for one implemented tool:
 * the descriptor and the invocation surface. The platform layer registers it
 * into the tool registry.
 */
final case class ToolImplementationHandle(
  descriptor: ToolBuildCtx => Either[ToolBuildError, ExtendedToolType],
  bindings: List[ToolMethodBinding],
  subtreeForwards: List[ToolSubtreeForward]
)

/** How one tool method parameter is supplied at invocation time. */
sealed trait ToolParamDecoder extends Product with Serializable
object ToolParamDecoder {

  /** Decoded from the canonical input field with the given surface name. */
  final case class Field(name: String, decode: SchemaValue => Either[String, Any]) extends ToolParamDecoder

  /** Auto-injected from the invocation principal. */
  case object PrincipalParam extends ToolParamDecoder

  /** Auto-injected from the invocation stdin stream. */
  case object StdinParam extends ToolParamDecoder

  /** Auto-injected process stdout handle (also returned in the result). */
  case object StdoutParam extends ToolParamDecoder
}

/**
 * The runtime interpreter of macro-generated tool invocation surfaces: command
 * path resolution, canonical input decoding, method binding dispatch, and
 * subtree forwarding — the Scala port of the invoke code the Rust
 * `#[tool_definition]` macro generates inline.
 */
object ToolInvokerRuntime {

  /**
   * The execution context used for the invoker's own result transformations.
   * All actual work happens in the caller's continuations, so the parasitic
   * (calling-thread) context is the right choice on both JVM and JS.
   */
  val executionContext: ExecutionContext = ExecutionContext.parasitic

  /** Builds the invocation handler for one registered tool. */
  def handler(
    tool: ExtendedToolType,
    handle: ToolImplementationHandle,
    env: ToolInvokeEnv
  ): ToolInvokeHandler =
    new ToolInvokeHandler {
      def invoke(
        commandPath: List[String],
        input: TypedSchemaValue,
        stdin: Option[ToolInputStream],
        principal: Principal
      ): Future[Either[ToolInvokeError, ToolInvokeResult]] =
        ToolInvokerRuntime.invoke(tool, handle, env, commandPath, input, stdin, principal)
    }

  def invoke(
    tool: ExtendedToolType,
    handle: ToolImplementationHandle,
    env: ToolInvokeEnv,
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream],
    principal: Principal
  ): Future[Either[ToolInvokeError, ToolInvokeResult]] =
    tool.commandIndexByPath(commandPath) match {
      case None               => failed(ToolInvokeError.InvalidCommandPath(commandPath))
      case Some(commandIndex) =>
        tool.decodeCanonicalInputRecord(commandIndex, input.value) match {
          case Left(err)     => failed(ToolInvokeError.InvalidInput(err.message))
          case Right(fields) =>
            val binding = handle.bindings.find { b =>
              tool.commandIndexByPath(b.commandPath).contains(commandIndex)
            }
            binding match {
              case Some(b) =>
                b.run(new ToolInvocationContext(fields, stdin, principal, env))
              case None =>
                handle.subtreeForwards.find(f => commandPath.startsWith(f.pathPrefix)) match {
                  case Some(forward) =>
                    forwardToSubtree(env, forward, commandPath, fields, input, stdin, principal)
                  case None =>
                    failed(ToolInvokeError.InvalidCommandPath(commandPath))
                }
            }
        }
    }

  /**
   * Forwards an invocation into a subtree child tool: the child's canonical
   * input record for the remaining path is reconstructed from the parent's
   * decoded fields (matching by surface name or alias, and requiring the exact
   * field schema), then the child's registered invoker is called.
   */
  private def forwardToSubtree(
    env: ToolInvokeEnv,
    forward: ToolSubtreeForward,
    commandPath: List[String],
    fields: List[CanonicalInputValue],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream],
    principal: Principal
  ): Future[Either[ToolInvokeError, ToolInvokeResult]] = {
    val subPath = commandPath.drop(forward.pathPrefix.length)
    env.invokerFor(forward.childToolName) match {
      case None          => failed(ToolInvokeError.InvalidToolName(forward.childToolName))
      case Some(invoker) =>
        env.extendedToolFor(forward.childToolName) match {
          case None          => invoker.invoke(subPath, input, stdin, principal)
          case Some(subtool) =>
            subtool.commandIndexByPath(subPath) match {
              case None           => failed(ToolInvokeError.InvalidCommandPath(commandPath))
              case Some(subIndex) =>
                val subFields = subtool.canonicalInputFields(subIndex)
                CanonicalInputModel.fromFields(subFields) match {
                  case Left(err)    => failed(ToolInvokeError.InvalidInput(err.message))
                  case Right(model) =>
                    val values = List.newBuilder[SchemaValue]
                    val it     = subFields.iterator
                    while (it.hasNext) {
                      val field = it.next()
                      val hit   = fields.find { candidate =>
                        candidate.name == field.name ||
                        candidate.aliases.contains(field.name) ||
                        field.aliases.exists(a => a == candidate.name || candidate.aliases.contains(a))
                      }
                      hit match {
                        case None =>
                          return failed(
                            ToolInvokeError.InvalidInput(
                              s"missing canonical tool input field `${field.name}`"
                            )
                          )
                        case Some(value) =>
                          if (value.schema != field.schema)
                            return failed(
                              ToolInvokeError.InvalidInput(
                                s"canonical tool input field `${value.name}` has incompatible " +
                                  s"schema for forwarded field `${field.name}`"
                              )
                            )
                          values += value.value
                      }
                    }
                    val subInput = TypedSchemaValue(
                      model.recordSchema,
                      SchemaValue.RecordValue(values.result())
                    )
                    invoker.invoke(subPath, subInput, stdin, principal)
                }
            }
        }
    }
  }

  /**
   * Resolves every method parameter for one invocation: canonical fields are
   * found by surface name and decoded, and Principal / stdin / stdout
   * parameters are injected. Returns the positional argument vector plus the
   * stdout handle when the method declared one.
   */
  def decodeArgs(
    ctx: ToolInvocationContext,
    decoders: List[ToolParamDecoder]
  ): Either[ToolInvokeError, (Vector[Any], Option[ToolOutputStream])] = {
    val args   = Vector.newBuilder[Any]
    var stdout = Option.empty[ToolOutputStream]
    val it     = decoders.iterator
    while (it.hasNext) {
      it.next() match {
        case ToolParamDecoder.Field(name, decode) =>
          ctx.fields.find(_.name == name) match {
            case None =>
              return Left(
                ToolInvokeError.InvalidInput(s"missing canonical tool input field `$name`")
              )
            case Some(field) =>
              decode(field.value) match {
                case Left(message) => return Left(ToolInvokeError.InvalidInput(message))
                case Right(value)  => args += value
              }
          }
        case ToolParamDecoder.PrincipalParam =>
          args += ctx.principal
        case ToolParamDecoder.StdinParam =>
          ctx.stdin match {
            case None =>
              return Left(
                ToolInvokeError.InvalidInput("tool invocation did not contain declared stdin stream")
              )
            case Some(stream) => args += stream
          }
        case ToolParamDecoder.StdoutParam =>
          val handle = ctx.env.stdout()
          stdout = Some(handle)
          args += handle
      }
    }
    Right((args.result(), stdout))
  }

  /** Field decoder used by [[ToolParamDecoder.Field]] entries. */
  def fieldDecoder[A](fromSchema: FromSchema[A]): SchemaValue => Either[String, Any] =
    value => fromSchema.fromValue(value).left.map(_.message)

  /**
   * Field decoder for count-flag parameters: the canonical field is a `u32`
   * (the flag's occurrence count), decoded into the implementation's `Int`
   * parameter.
   */
  def countFlagDecoder: SchemaValue => Either[String, Any] = {
    case SchemaValue.U32Value(v) => Right(v.toInt)
    case SchemaValue.S32Value(v) => Right(v)
    case other                   => Left(s"count flag value must be a u32, found: $other")
  }

  def encodeSuccess[A](
    value: A,
    intoSchema: IntoSchema[A],
    stdout: Option[ToolOutputStream]
  ): Either[ToolInvokeError, ToolInvokeResult] =
    try Right(ToolInvokeResult(Some(intoSchema.toTyped(value)), stdout))
    catch {
      case e: SchemaEncodeError => Left(ToolInvokeError.InvalidResult(e.message))
    }

  def encodeUnit(stdout: Option[ToolOutputStream]): Either[ToolInvokeError, ToolInvokeResult] =
    Right(ToolInvokeResult(None, stdout))

  /**
   * Encodes a declared tool error value (the `Left` of an `Either[E, T]`
   * result) into the custom-error payload carrier.
   */
  def customError[E](error: E, schema: ToolErrorSchema[E]): ToolInvokeError =
    schema.toErrorPayloadValue(error) match {
      case Right(payload) => ToolInvokeError.Custom(payload)
      case Left(message)  => ToolInvokeError.InvalidResult(message)
    }

  private def failed[T](error: ToolInvokeError): Future[Either[ToolInvokeError, T]] =
    Future.successful(Left(error))
}
