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

import golem.schema.{FromSchema, IntoSchema, SchemaEncodeError, SchemaValue, TypedSchemaValue}

import scala.collection.mutable
import scala.concurrent.Future

/** RPC-level failures reported while invoking a remote tool. */
sealed trait RpcError extends Product with Serializable {
  def message: String = this match {
    case RpcError.Protocol(m)       => s"protocol error: $m"
    case RpcError.Denied(m)         => s"denied: $m"
    case RpcError.NotFound(m)       => s"not found: $m"
    case RpcError.RemoteInternal(m) => s"remote internal error: $m"
  }
}

object RpcError {
  final case class Protocol(detail: String)       extends RpcError
  final case class Denied(detail: String)         extends RpcError
  final case class NotFound(detail: String)       extends RpcError
  final case class RemoteInternal(detail: String) extends RpcError
}

/** Failure returned by a typed tool client. */
sealed trait ToolError[+E] extends Product with Serializable
object ToolError {
  final case class Rpc(error: RpcError) extends ToolError[Nothing]
  final case class Tool[E](error: E)    extends ToolError[E]
}

/**
 * Platform-neutral mirror of the wire `rpc-error` returned by
 * `golem:tool/host`'s `tool-rpc` resource. The platform layer converts the
 * thrown wire error into this form at the host-import boundary.
 */
sealed trait ToolRpcFailure extends Product with Serializable
object ToolRpcFailure {
  final case class ProtocolError(message: String)          extends ToolRpcFailure
  final case class Denied(message: String)                 extends ToolRpcFailure
  final case class NotFound(message: String)               extends ToolRpcFailure
  final case class RemoteInternalError(message: String)    extends ToolRpcFailure
  final case class RemoteToolError(error: ToolInvokeError) extends ToolRpcFailure
}

/**
 * The transport a typed tool client invokes through: one remote tool's
 * `invoke-and-await` entry point, expressed over the platform-neutral model
 * types. The platform layer supplies the real implementation (backed by the
 * `golem:tool/host` `tool-rpc` resource); tests may use fakes.
 */
trait ToolRpcTransport {
  def invokeAndAwait(
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream]
  ): Future[Either[ToolRpcFailure, ToolInvokeResult]]
}

/**
 * The runtime layer of generated typed tool clients: invocation with
 * RPC/custom-error mapping, canonical input assembly (static and
 * inherited-prefix dynamic), and result decoding — the Scala port of the Rust
 * SDK's `tool_client.rs` plus the invoke code the Rust client macro generates
 * inline.
 */
object ToolClientRuntime {

  private implicit val ec: scala.concurrent.ExecutionContext =
    ToolInvokerRuntime.executionContext

  // -------------------------------------------------------------------------
  // Invocation with error mapping
  // -------------------------------------------------------------------------

  /** Invokes a tool and decodes remote custom errors with the given decoder. */
  def invokeAndAwait[E](
    rpc: ToolRpcTransport,
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream],
    decodeError: TypedSchemaValue => Either[String, E]
  ): Future[Either[ToolError[E], ToolInvokeResult]] =
    rpc.invokeAndAwait(commandPath, input, stdin).map {
      case Right(result) => Right(result)
      case Left(failure) => Left(mapRpcFailure(failure, decodeError))
    }

  /**
   * Invokes a tool whose remote custom-error payload is directly encoded as
   * `E`.
   */
  def invokeAndAwaitPayloadError[E](
    rpc: ToolRpcTransport,
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream]
  )(implicit from: FromSchema[E]): Future[Either[ToolError[E], ToolInvokeResult]] =
    invokeAndAwait[E](rpc, commandPath, input, stdin, decodeCustomToolError[E](_))

  /**
   * Invokes a zero-error tool, treating remote custom errors as protocol
   * failures.
   */
  def invokeAndAwaitInfallible(
    rpc: ToolRpcTransport,
    commandPath: List[String],
    input: TypedSchemaValue,
    stdin: Option[ToolInputStream]
  ): Future[Either[ToolError[Nothing], ToolInvokeResult]] =
    rpc.invokeAndAwait(commandPath, input, stdin).map {
      case Right(result) => Right(result)
      case Left(failure) => Left(mapInfallibleFailure(failure))
    }

  private def mapRpcFailure[E](
    failure: ToolRpcFailure,
    decodeError: TypedSchemaValue => Either[String, E]
  ): ToolError[E] =
    failure match {
      case ToolRpcFailure.ProtocolError(m)       => ToolError.Rpc(RpcError.Protocol(m))
      case ToolRpcFailure.Denied(m)              => ToolError.Rpc(RpcError.Denied(m))
      case ToolRpcFailure.NotFound(m)            => ToolError.Rpc(RpcError.NotFound(m))
      case ToolRpcFailure.RemoteInternalError(m) => ToolError.Rpc(RpcError.RemoteInternal(m))
      case ToolRpcFailure.RemoteToolError(error) => mapRemoteToolError(error, decodeError)
    }

  private def mapInfallibleFailure(failure: ToolRpcFailure): ToolError[Nothing] =
    failure match {
      case ToolRpcFailure.ProtocolError(m)       => ToolError.Rpc(RpcError.Protocol(m))
      case ToolRpcFailure.Denied(m)              => ToolError.Rpc(RpcError.Denied(m))
      case ToolRpcFailure.NotFound(m)            => ToolError.Rpc(RpcError.NotFound(m))
      case ToolRpcFailure.RemoteInternalError(m) => ToolError.Rpc(RpcError.RemoteInternal(m))
      case ToolRpcFailure.RemoteToolError(error) =>
        ToolError.Rpc(RpcError.Protocol(s"remote tool error: ${remoteToolErrorLabel(error)}"))
    }

  private[tool] def mapRemoteToolError[E](
    error: ToolInvokeError,
    decodeError: TypedSchemaValue => Either[String, E]
  ): ToolError[E] =
    error match {
      case ToolInvokeError.Custom(payload) =>
        decodeError(payload) match {
          case Right(decoded) => ToolError.Tool(decoded)
          case Left(message)  => ToolError.Rpc(RpcError.Protocol(message))
        }
      case other =>
        ToolError.Rpc(RpcError.Protocol(s"remote tool error: ${remoteToolErrorLabel(other)}"))
    }

  private[tool] def decodeCustomToolError[E](
    value: TypedSchemaValue
  )(implicit from: FromSchema[E]): Either[String, E] =
    from.fromValue(value.value).left.map(e => s"failed to decode remote tool error: ${e.message}")

  private def remoteToolErrorLabel(error: ToolInvokeError): String =
    error match {
      case ToolInvokeError.InvalidToolName(name)        => s"invalid tool name `$name`"
      case ToolInvokeError.InvalidCommandPath(path)     => s"invalid command path `${path.mkString(" ")}`"
      case ToolInvokeError.InvalidInput(message)        => s"invalid input: $message"
      case ToolInvokeError.ConstraintViolation(message) => s"constraint violation: $message"
      case ToolInvokeError.InvalidResult(message)       => s"invalid result: $message"
      case ToolInvokeError.Custom(_)                    => "custom error"
    }

  // -------------------------------------------------------------------------
  // Generated-client helpers: parameter encoding
  // -------------------------------------------------------------------------

  def protocolError(message: String): ToolError[Nothing] =
    ToolError.Rpc(RpcError.Protocol(message))

  /**
   * Evaluates the generated parameter-value list, converting a schema encode
   * failure into a protocol error.
   */
  def encodeParams(
    build: => List[(String, SchemaValue)]
  ): Either[ToolError[Nothing], List[(String, SchemaValue)]] =
    try Right(build)
    catch {
      case e: SchemaEncodeError =>
        Left(protocolError(s"failed to encode tool parameter: ${e.message}"))
    }

  /**
   * The canonical value of a count-flag parameter: the wire field is a `u32`
   * occurrence count, while the implementation parameter is an `Int`.
   */
  def countFlagValue(count: Int): SchemaValue =
    SchemaValue.U32Value(count.toLong)

  /** The canonical value graph of a count-flag field (a `u32`). */
  val countFlagGraph: golem.schema.SchemaGraph =
    golem.schema.SchemaGraph(
      scala.collection.immutable.ListMap.empty,
      golem.schema.SchemaType(golem.schema.SchemaTypeBody.U32Type(None))
    )

  /** One inherited canonical-prefix entry a subtree navigation packs. */
  def prefixValue[A](
    name: String,
    aliases: List[String],
    value: A,
    into: IntoSchema[A]
  ): CanonicalInputValue =
    CanonicalInputValue(name, aliases, into.graph, into.toValue(value))

  /** An inherited canonical-prefix entry for a count-flag parameter. */
  def countFlagPrefixValue(name: String, aliases: List[String], count: Int): CanonicalInputValue =
    CanonicalInputValue(name, aliases, countFlagGraph, countFlagValue(count))

  // -------------------------------------------------------------------------
  // Generated-client helpers: canonical input assembly
  // -------------------------------------------------------------------------

  /**
   * Resolves the canonical input model of one generated command against the
   * tool's descriptor; used to back the per-method static model caches of
   * generated client companions.
   */
  def staticInputModel(
    descriptor: Either[ToolBuildError, ExtendedToolType],
    schemaPath: List[String]
  ): Either[String, CanonicalInputModel] =
    descriptor.left.map(e => s"tool descriptor build failed: ${e.message}").flatMap { tool =>
      tool.commandIndexByPath(schemaPath) match {
        case None =>
          Left(s"invalid generated tool command path `${schemaPath.mkString(" ")}`")
        case Some(index) =>
          tool.canonicalInputModel(index).left.map(_.message)
      }
    }

  /**
   * Builds the invocation input record from a static canonical model: the fast
   * path applies when the generated parameter values already align with the
   * canonical field order, otherwise values are matched by field name.
   */
  def buildInputFromModel(
    model: Either[String, CanonicalInputModel],
    paramValues: List[(String, SchemaValue)]
  ): Either[ToolError[Nothing], TypedSchemaValue] =
    model match {
      case Left(message) => Left(protocolError(message))
      case Right(m)      =>
        val aligned =
          m.fields.length == paramValues.length &&
            m.fields.iterator.zip(paramValues.iterator).forall { case (field, (name, _)) =>
              field.name == name
            }
        if (aligned)
          Right(TypedSchemaValue(m.recordSchema, SchemaValue.RecordValue(paramValues.map(_._2))))
        else
          reorderValues(m.fields, paramValues).map { values =>
            TypedSchemaValue(m.recordSchema, SchemaValue.RecordValue(values))
          }
    }

  /**
   * Builds the invocation input record when an inherited canonical prefix is in
   * play (a subtree-navigated call): the model is synthesized from the prefix
   * fields plus the command's own canonical fields (minus those the prefix
   * already covers), and the record starts with the prefix values.
   */
  def buildDynamicInput(
    descriptor: Either[ToolBuildError, ExtendedToolType],
    schemaPath: List[String],
    inheritedPrefix: List[CanonicalInputValue],
    paramValues: List[(String, SchemaValue)]
  ): Either[ToolError[Nothing], TypedSchemaValue] =
    descriptor match {
      case Left(error) =>
        Left(protocolError(s"tool descriptor build failed: ${error.message}"))
      case Right(tool) =>
        tool.commandIndexByPath(schemaPath) match {
          case None =>
            Left(protocolError(s"invalid generated tool command path `${schemaPath.mkString(" ")}`"))
          case Some(index) =>
            val inheritedFields             = inheritedPrefix.map(v => CanonicalInputField(v.name, v.aliases, v.schema))
            val inheritedNames: Set[String] =
              inheritedPrefix.flatMap(v => v.name :: v.aliases).toSet
            val ownFields = tool.canonicalInputFields(index).filterNot { field =>
              inheritedNames.contains(field.name) || field.aliases.exists(inheritedNames.contains)
            }
            CanonicalInputModel.fromFields(inheritedFields ++ ownFields) match {
              case Left(error)  => Left(protocolError(error.message))
              case Right(model) =>
                reorderValues(model.fields.drop(inheritedPrefix.length), paramValues).map { rest =>
                  TypedSchemaValue(
                    model.recordSchema,
                    SchemaValue.RecordValue(inheritedPrefix.map(_.value) ++ rest)
                  )
                }
            }
        }
    }

  /**
   * Matches parameter values to canonical fields by name, consuming duplicate
   * surface names from the last occurrence backwards (mirroring the Rust
   * client's `rposition` matching).
   */
  private def reorderValues(
    fields: List[CanonicalInputField],
    paramValues: List[(String, SchemaValue)]
  ): Either[ToolError[Nothing], List[SchemaValue]] = {
    val remaining = mutable.ArrayBuffer.from(paramValues)
    val out       = List.newBuilder[SchemaValue]
    val it        = fields.iterator
    while (it.hasNext) {
      val field = it.next()
      val index = remaining.lastIndexWhere(_._1 == field.name)
      if (index < 0)
        return Left(protocolError(s"missing canonical tool input field `${field.name}`"))
      out += remaining.remove(index)._2
    }
    Right(out.result())
  }

  // -------------------------------------------------------------------------
  // Generated-client helpers: invocation entry points
  // -------------------------------------------------------------------------

  /**
   * Runs one generated typed client call once its input record is assembled.
   */
  def run[E](
    rpc: ToolRpcTransport,
    commandPath: List[String],
    input: Either[ToolError[Nothing], TypedSchemaValue],
    stdin: Option[ToolInputStream],
    decodeError: TypedSchemaValue => Either[String, E]
  ): Future[Either[ToolError[E], ToolInvokeResult]] =
    input match {
      case Left(error)   => Future.successful(Left(error))
      case Right(record) => invokeAndAwait(rpc, commandPath, record, stdin, decodeError)
    }

  /**
   * Applies a generated result decoder to a completed typed client call
   * (keeping the generated code free of explicit execution-context plumbing).
   */
  def complete[E, T](
    call: Future[Either[ToolError[E], ToolInvokeResult]]
  )(decode: ToolInvokeResult => Either[ToolError[E], T]): Future[Either[ToolError[E], T]] =
    call.map(_.flatMap(decode))

  /** Runs one generated zero-error typed client call. */
  def runInfallible(
    rpc: ToolRpcTransport,
    commandPath: List[String],
    input: Either[ToolError[Nothing], TypedSchemaValue],
    stdin: Option[ToolInputStream]
  ): Future[Either[ToolError[Nothing], ToolInvokeResult]] =
    input match {
      case Left(error)   => Future.successful(Left(error))
      case Right(record) => invokeAndAwaitInfallible(rpc, commandPath, record, stdin)
    }

  // -------------------------------------------------------------------------
  // Generated-client helpers: result decoding
  // -------------------------------------------------------------------------

  def decodeUnitResult(result: ToolInvokeResult): Either[ToolError[Nothing], Unit] =
    for {
      _ <- requireNoStdout(result)
      _ <- requireNoValue(result)
    } yield ()

  def decodeValueResult[T](
    result: ToolInvokeResult,
    from: FromSchema[T]
  ): Either[ToolError[Nothing], T] =
    for {
      _       <- requireNoStdout(result)
      decoded <- requireValue(result, from)
    } yield decoded

  def decodeStdoutResult(result: ToolInvokeResult): Either[ToolError[Nothing], ToolOutputStream] =
    for {
      stdout <- requireStdout(result)
      _      <- requireNoValue(result)
    } yield stdout

  def decodeValueStdoutResult[T](
    result: ToolInvokeResult,
    from: FromSchema[T]
  ): Either[ToolError[Nothing], (T, ToolOutputStream)] =
    for {
      stdout  <- requireStdout(result)
      decoded <- requireValue(result, from)
    } yield (decoded, stdout)

  private def requireStdout(result: ToolInvokeResult): Either[ToolError[Nothing], ToolOutputStream] =
    result.stdout.toRight(protocolError("tool result did not contain declared stdout stream"))

  private def requireNoStdout(result: ToolInvokeResult): Either[ToolError[Nothing], Unit] =
    if (result.stdout.isDefined)
      Left(protocolError("tool result unexpectedly contained stdout stream"))
    else Right(())

  private def requireValue[T](
    result: ToolInvokeResult,
    from: FromSchema[T]
  ): Either[ToolError[Nothing], T] =
    result.result match {
      case None        => Left(protocolError("tool result did not contain a value"))
      case Some(value) =>
        from.fromValue(value.value).left.map(e => protocolError(e.message))
    }

  private def requireNoValue(result: ToolInvokeResult): Either[ToolError[Nothing], Unit] =
    if (result.result.isDefined)
      Left(protocolError("tool result unexpectedly contained a value"))
    else Right(())
}
