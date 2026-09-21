/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.reflection

import golem.host.SchemaWireInterop
import golem.host.js.{JsComponentId, JsUuid}
import golem.host.js.schema.{
  JsInputSchema,
  JsNamedField,
  JsOutputSchema,
  JsSchemaGraph,
  JsSchemaValueTree,
  JsTypedSchemaValue,
  JsUuid => JsSchemaUuid
}
import golem.runtime.rpc.host.{AgentHostApi, WasmRpcApi}
import golem.runtime.rpc.{CancellationToken, InvocationReceipt}
import golem.runtime.tool.host.ToolHostApi
import golem.schema._
import golem.schema.SchemaTypeBody.RecordType
import golem.schema.validation.ValueValidation
import golem.schema.wire.SchemaWire
import golem.config.ConfigOverride
import golem.{Datetime, FutureInterop, Uuid}
import zio.blocks.schema.json.Json

import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.util.control.NonFatal

import ReflectionInternals._

sealed trait AgentMode extends Product with Serializable
object AgentMode {
  case object Durable   extends AgentMode
  case object Ephemeral extends AgentMode
}

final case class ComponentId(uuid: Uuid) {
  private[golem] def toJs: JsComponentId =
    JsComponentId(JsUuid(js.BigInt(uuid.highBits.toString), js.BigInt(uuid.lowBits.toString)))
}

object ComponentId {
  private[golem] def fromJs(value: JsComponentId): ComponentId =
    ComponentId(Uuid(BigInt(value.uuid.highBits.toString), BigInt(value.uuid.lowBits.toString)))
}

final case class ParsedAgentId(value: String) {
  def parts: Either[GolemReflectError, ParsedAgentIdParts]         = ParsedAgentId.parse(this)
  def dynamicClient: Either[GolemReflectError, DynamicAgentClient] = DynamicAgentClient.fromAgentId(this)
  def client[Capability <: AgentClientCapability, Constructor, Config](
    definition: AgentClientDefinition[Capability, Constructor, Config]
  )(implicit canBind: CanBindAgentClient[Capability]): Either[GolemReflectError, CallerCodecAgentClient] =
    definition.bind(this)

  def clientWithOverrides[Capability <: AgentClientCapability, Constructor, Config](
    definition: AgentClientDefinition[Capability, Constructor, Config],
    overrides: List[ConfigOverride]
  )(implicit canBind: CanBindAgentClient[Capability]): Either[GolemReflectError, CallerCodecAgentClient] =
    definition.bindWithOverrides(this, overrides)

  def clientWithConfig[Capability <: AgentClientCapability, Constructor, Config](
    definition: AgentClientDefinition[Capability, Constructor, Config],
    config: Config
  )(implicit
    full: Capability <:< Full,
    canBind: CanBindAgentClient[Capability]
  ): Either[GolemReflectError, CallerCodecAgentClient] =
    definition.bindWithConfig(this, config)
}

final case class ParsedAgentIdParts(typeName: String, constructorValue: SchemaValue, phantomId: Option[Uuid])

object ParsedAgentId {
  def create(
    typeName: String,
    constructorValue: SchemaValue,
    phantomId: Option[Uuid] = None
  ): Either[GolemReflectError, ParsedAgentId] =
    encode(constructorValue).flatMap(payload =>
      AgentHostApi
        .makeAgentId(typeName, payload, phantomId)
        .left
        .map(GolemReflectError.Identity.apply)
        .map(ParsedAgentId(_))
    )

  def parse(agentId: ParsedAgentId): Either[GolemReflectError, ParsedAgentIdParts] =
    AgentHostApi
      .parseAgentId(agentId.value)
      .left
      .map(GolemReflectError.Identity.apply)
      .flatMap { parts =>
        try
          Right(
            ParsedAgentIdParts(
              parts.agentTypeName,
              SchemaWire.schemaValueFromWit(SchemaWireInterop.valueTreeFromJs(parts.payload.value)),
              parts.phantom
            )
          )
        catch { case NonFatal(error) => Left(GolemReflectError.SchemaDecode(error.getMessage)) }
      }
}

sealed trait GolemReflectError extends Product with Serializable {
  def message: String
  override def toString: String = message
}

object GolemReflectError {
  final case class Discovery(message: String)    extends GolemReflectError
  final case class Identity(message: String)     extends GolemReflectError
  final case class SchemaEncode(message: String) extends GolemReflectError
  final case class SchemaDecode(message: String) extends GolemReflectError
  final case class Validation(message: String)   extends GolemReflectError
  final case class Remote(error: AgentRpcError)  extends GolemReflectError {
    val message: String = error.message
  }
}

sealed trait AgentRpcError extends Product with Serializable { def message: String }
object AgentRpcError {
  final case class Protocol(detail: String)       extends AgentRpcError { val message = s"protocol-error: $detail" }
  final case class Denied(detail: String)         extends AgentRpcError { val message = s"denied: $detail"         }
  final case class NotFound(detail: String)       extends AgentRpcError { val message = s"not-found: $detail"      }
  final case class RemoteInternal(detail: String) extends AgentRpcError {
    val message = s"remote-internal-error: $detail"
  }
  final case class InvalidInput(detail: String)                  extends AgentRpcError { val message = s"invalid-input: $detail"    }
  final case class InvalidMethod(detail: String)                 extends AgentRpcError { val message = s"invalid-method: $detail"   }
  final case class InvalidType(detail: String)                   extends AgentRpcError { val message = s"invalid-type: $detail"     }
  final case class InvalidAgentId(detail: String)                extends AgentRpcError { val message = s"invalid-agent-id: $detail" }
  final case class Custom(payload: TypedSchemaValue)             extends AgentRpcError { val message = "custom-error"               }
  final case class Unknown(kind: String, detail: Option[String]) extends AgentRpcError {
    val message: String = detail.fold(kind)(value => s"$kind: $value")
  }
}

final case class AgentMethod(
  name: String,
  description: String,
  promptHint: Option[String],
  input: SchemaRef,
  output: Option[SchemaRef]
)

final class AgentType private[reflection] (
  val name: String,
  val description: String,
  val sourceLanguage: String,
  val mode: AgentMode,
  val implementedBy: ComponentId,
  val constructorInput: SchemaRef,
  val methods: List[AgentMethod],
  val config: List[ReflectedConfigDeclaration]
) {
  val client: ReflectedAgentClientFactory = new ReflectedAgentClientFactory(this)

  def method(name: String): Option[AgentMethod] = methods.find(_.name == name)

  def agentId(input: Json, phantomId: Option[Uuid] = None): Either[GolemReflectError, ParsedAgentId] =
    constructorInput
      .packJson(input)
      .left
      .map(error => GolemReflectError.Validation(error.message))
      .flatMap(agentIdValue(_, phantomId))

  def agentIdValue(input: SchemaValue, phantomId: Option[Uuid] = None): Either[GolemReflectError, ParsedAgentId] =
    validate(constructorInput, input).flatMap(_ => ParsedAgentId.create(name, input, phantomId))

  def bind(
    agentId: ParsedAgentId,
    overrides: List[ConfigOverride] = Nil
  ): Either[GolemReflectError, ReflectedAgentClient] =
    for {
      parts <- agentId.parts
      _     <- Either.cond(
             parts.typeName == name,
             (),
             GolemReflectError.Identity(s"Agent type '$name' cannot bind '${parts.typeName}'")
           )
      _ <- Either.cond(
             mode == AgentMode.Durable,
             (),
             GolemReflectError.Identity(s"Cannot bind an existing identity to ephemeral agent type '$name'")
           )
      client <- client.createValue(parts.constructorValue, parts.phantomId, overrides)
    } yield client

  def packConfigJson(entries: List[ReflectedConfigJson]): Either[GolemReflectError, List[ConfigOverride]] =
    ReflectionInternals.sequence(entries.map { entry =>
      configDeclaration(entry.path).flatMap { declaration =>
        declaration.schema
          .packJson(entry.value)
          .left
          .map(error => GolemReflectError.Validation(error.message))
          .flatMap(value => validate(declaration.schema, value).map(_ => value))
          .map(value => ConfigOverride(entry.path, TypedSchemaValue(declaration.schema.graph, value)))
      }
    })

  def validateConfig(overrides: List[ConfigOverride]): Either[GolemReflectError, List[ConfigOverride]] =
    ReflectionInternals.sequence(overrides.map { entry =>
      configDeclaration(entry.path).flatMap { declaration =>
        validate(declaration.schema, entry.value.value).map { _ =>
          ConfigOverride(entry.path, TypedSchemaValue(declaration.schema.graph, entry.value.value))
        }
      }
    })

  private def configDeclaration(path: List[String]): Either[GolemReflectError, ReflectedConfigDeclaration] =
    config.find(_.path == path) match {
      case None                                                => Left(GolemReflectError.Validation(s"Unknown config path '${path.mkString(".")}'"))
      case Some(declaration) if declaration.source == "secret" =>
        Left(GolemReflectError.Validation(s"Cannot override secret config field '${path.mkString(".")}' over RPC"))
      case Some(declaration) => Right(declaration)
    }
}

final case class ReflectedConfigDeclaration(path: List[String], source: String, schema: SchemaRef)
final case class ReflectedConfigJson(path: List[String], value: Json)

object Reflection {
  def getAllToolTypes(): Either[GolemReflectError, List[ToolType]] =
    try sequence(ToolHostApi.getAllTools().map(ToolType.fromRegistered))
    catch { case NonFatal(error) => Left(GolemReflectError.Discovery(error.getMessage)) }

  def getToolType(name: String): Either[GolemReflectError, ToolType] =
    try
      ToolHostApi
        .getTool(name)
        .toRight(GolemReflectError.Discovery(s"Tool '$name' was not found"))
        .flatMap(ToolType.fromRegistered)
    catch { case NonFatal(error) => Left(GolemReflectError.Discovery(error.getMessage)) }

  def getAllAgentTypes(): Either[GolemReflectError, List[AgentType]] =
    try sequence(AgentHostApi.getAllAgentTypes().map(decodeAgentType))
    catch { case NonFatal(error) => Left(GolemReflectError.Discovery(error.getMessage)) }

  def getAgentType(name: String): Either[GolemReflectError, Option[AgentType]] =
    try
      AgentHostApi
        .registeredAgentType(name)
        .map(decodeAgentType)
        .fold[Either[GolemReflectError, Option[AgentType]]](Right(None))(_.map(Some(_)))
    catch { case NonFatal(error) => Left(GolemReflectError.Discovery(error.getMessage)) }

  def getAgentTypeFor(agentId: ParsedAgentId): Either[GolemReflectError, Option[AgentType]] =
    try
      AgentHostApi
        .registeredAgentTypeFor(agentId.value)
        .map(decodeAgentType)
        .fold[Either[GolemReflectError, Option[AgentType]]](Right(None))(_.map(Some(_)))
    catch { case NonFatal(error) => Left(GolemReflectError.Discovery(error.getMessage)) }

  private def decodeAgentType(registered: AgentHostApi.RegisteredAgentType): Either[GolemReflectError, AgentType] =
    try {
      val raw     = registered.agentType
      val graph   = raw.schema
      val decoded = SchemaWire.schemaGraphFromWit(SchemaWireInterop.graphFromJs(graph))
      val methods = raw.methods.toList.map { method =>
        AgentMethod(
          method.name,
          method.description,
          method.promptHint.toOption,
          inputRef(graph, decoded, method.inputSchema),
          outputRef(graph, decoded, method.outputSchema)
        )
      }
      val mode = raw.mode match {
        case "durable"   => AgentMode.Durable
        case "ephemeral" => AgentMode.Ephemeral
        case other       => throw new IllegalArgumentException(s"unknown agent mode '$other'")
      }
      Right(
        new AgentType(
          raw.typeName,
          raw.description,
          raw.sourceLanguage,
          mode,
          ComponentId.fromJs(registered.implementedBy),
          inputRef(graph, decoded, raw.constructor.inputSchema),
          methods,
          raw.config.toList.map { declaration =>
            val rooted = SchemaWire
              .schemaGraphFromWit(SchemaWireInterop.graphFromJs(graph).copy(root = declaration.valueType))
            ReflectedConfigDeclaration(declaration.path.toList, declaration.source, SchemaRef(rooted))
          }
        )
      )
    } catch { case NonFatal(error) => Left(GolemReflectError.SchemaDecode(error.getMessage)) }

  private def inputRef(graph: JsSchemaGraph, decoded: SchemaGraph, input: JsInputSchema): SchemaRef = {
    if (input.tag != "parameters") throw new IllegalArgumentException(s"unknown input schema '${input.tag}'")
    val entries = input.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[js.Array[JsNamedField]].toList
    val fields  = entries.collect {
      case entry if entry.source.tag == "user-supplied" =>
        val root = SchemaWire.schemaGraphFromWit(SchemaWireInterop.graphFromJs(graph).copy(root = entry.schema)).root
        NamedFieldType(entry.name, root, SchemaWireInterop.metadataFromJs(entry.metadata))
    }
    SchemaRef(SchemaGraph(decoded.defs, SchemaType(RecordType(fields))))
  }

  private def outputRef(graph: JsSchemaGraph, decoded: SchemaGraph, output: JsOutputSchema): Option[SchemaRef] =
    output.tag match {
      case "unit"   => None
      case "single" =>
        val root   = output.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[Int]
        val rooted = SchemaWire.schemaGraphFromWit(SchemaWireInterop.graphFromJs(graph).copy(root = root)).root
        Some(SchemaRef(decoded, rooted))
      case other => throw new IllegalArgumentException(s"unknown output schema '$other'")
    }
}

final case class ReflectedPhantomClient(agentId: ParsedAgentId, phantomId: Uuid, client: ReflectedAgentClient)

final class ReflectedAgentClientFactory private[reflection] (agentType: AgentType) {
  def get(input: Json, config: List[ReflectedConfigJson] = Nil): Either[GolemReflectError, ReflectedAgentClient] =
    for {
      _           <- requireDurable("get")
      constructor <- pack(input)
      overrides   <- agentType.packConfigJson(config)
      client      <- createValue(constructor, None, overrides)
    } yield client

  def getValue(
    input: SchemaValue,
    config: List[ConfigOverride] = Nil
  ): Either[GolemReflectError, ReflectedAgentClient] =
    requireDurable("getValue").flatMap(_ => createValue(input, None, config))

  def getPhantom(
    input: Json,
    phantomId: Uuid,
    config: List[ReflectedConfigJson] = Nil
  ): Either[GolemReflectError, ReflectedAgentClient] =
    for {
      constructor <- pack(input)
      overrides   <- agentType.packConfigJson(config)
      client      <- createValue(constructor, Some(phantomId), overrides)
    } yield client

  def getPhantomValue(
    input: SchemaValue,
    phantomId: Uuid,
    config: List[ConfigOverride] = Nil
  ): Either[GolemReflectError, ReflectedAgentClient] =
    createValue(input, Some(phantomId), config)

  def newPhantom(
    input: Json,
    config: List[ReflectedConfigJson] = Nil
  ): Either[GolemReflectError, Either[ReflectedAgentClient, ReflectedPhantomClient]] =
    for {
      constructor <- pack(input)
      overrides   <- agentType.packConfigJson(config)
      result      <- newPhantomValue(constructor, overrides)
    } yield result

  def newPhantomValue(
    input: SchemaValue,
    config: List[ConfigOverride] = Nil
  ): Either[GolemReflectError, Either[ReflectedAgentClient, ReflectedPhantomClient]] =
    if (agentType.mode == AgentMode.Ephemeral) createValue(input, None, config).map(Left(_))
    else {
      val phantom = Uuid.random()
      for {
        id     <- agentType.agentIdValue(input, Some(phantom))
        client <- createValue(input, Some(phantom), config)
      } yield Right(ReflectedPhantomClient(id, phantom, client))
    }

  private[reflection] def createValue(
    input: SchemaValue,
    phantomId: Option[Uuid],
    config: List[ConfigOverride] = Nil
  ): Either[GolemReflectError, ReflectedAgentClient] =
    validate(agentType.constructorInput, input)
      .flatMap(_ => agentType.validateConfig(config))
      .flatMap(overrides => Transport.create(agentType.name, input, phantomId, overrides))
      .map(new ReflectedAgentClient(agentType, _))

  private def pack(input: Json): Either[GolemReflectError, SchemaValue] =
    agentType.constructorInput.packJson(input).left.map(error => GolemReflectError.Validation(error.message))

  private def requireDurable(operation: String): Either[GolemReflectError, Unit] =
    Either.cond(
      agentType.mode == AgentMode.Durable,
      (),
      GolemReflectError.Identity(s"$operation is not available for ephemeral agent types")
    )
}

final class ReflectedAgentClient private[reflection] (agentType: AgentType, transport: Transport) {
  def method(name: String): Either[GolemReflectError, ReflectedAgentMethod] =
    agentType
      .method(name)
      .toRight(GolemReflectError.Discovery(s"Agent type '${agentType.name}' has no method '$name'"))
      .map(new ReflectedAgentMethod(_, transport))
}

final class ReflectedAgentMethod private[reflection] (val definition: AgentMethod, transport: Transport) {
  def invoke(input: Json): Future[Either[GolemReflectError, Invocation[Json]]] = invokeJson(input)

  def invokeJson(input: Json): Future[Either[GolemReflectError, Invocation[Json]]] =
    definition.input.packJson(input) match {
      case Left(error)  => Future.successful(Left(GolemReflectError.Validation(error.message)))
      case Right(value) =>
        invokeValue(value).map(_.flatMap { invocation =>
          invocation.value match {
            case None         => Right(Invocation(invocation.metadata, None))
            case Some(result) =>
              definition.output
                .toRight(GolemReflectError.SchemaDecode("unit method returned a value"))
                .flatMap(_.unpackJson(result).left.map(error => GolemReflectError.SchemaDecode(error.message)))
                .map(json => Invocation(invocation.metadata, Some(json)))
          }
        })
    }

  def invokeValue(input: SchemaValue): Future[Either[GolemReflectError, Invocation[SchemaValue]]] =
    validate(definition.input, input) match {
      case Left(error) => Future.successful(Left(error))
      case Right(_)    =>
        transport.invokeAndAwait(definition.name, input).map(_.flatMap(validateInvocationOutput(definition, _)))
    }

  def triggerValue(input: SchemaValue): Either[GolemReflectError, InvocationMetadata] =
    rejectNonAwaitedStreams("trigger")
      .flatMap(_ => validate(definition.input, input))
      .flatMap(_ => transport.trigger(definition.name, input))

  def triggerJson(input: Json): Either[GolemReflectError, InvocationMetadata] =
    definition.input
      .packJson(input)
      .left
      .map(error => GolemReflectError.Validation(error.message))
      .flatMap(triggerValue)

  def scheduleValue(at: Datetime, input: SchemaValue): Either[GolemReflectError, ScheduledInvocation] =
    rejectNonAwaitedStreams("schedule")
      .flatMap(_ => validate(definition.input, input))
      .flatMap(_ => transport.schedule(at, definition.name, input))

  def scheduleJson(at: Datetime, input: Json): Either[GolemReflectError, ScheduledInvocation] =
    definition.input
      .packJson(input)
      .left
      .map(error => GolemReflectError.Validation(error.message))
      .flatMap(scheduleValue(at, _))

  private def rejectNonAwaitedStreams(operation: String): Either[GolemReflectError, Unit] =
    Either.cond(
      !definition.input.containsStream && !definition.output.exists(_.containsStream),
      (),
      GolemReflectError.Validation(s"$operation is unavailable for streaming method '${definition.name}'")
    )
}

final case class InvocationMetadata(agentId: ParsedAgentId, idempotencyKey: String)
final case class Invocation[+A](metadata: InvocationMetadata, value: Option[A])
final case class ScheduledInvocation(metadata: InvocationMetadata, cancellationToken: CancellationToken)

final class DynamicAgentClient private (transport: Transport, val agentId: Option[ParsedAgentId]) {
  def method(name: String): DynamicAgentMethod = new DynamicAgentMethod(name, transport)
}

object DynamicAgentClient {
  def fromAgentId(agentId: ParsedAgentId): Either[GolemReflectError, DynamicAgentClient] =
    agentId.parts
      .flatMap(parts => Transport.create(parts.typeName, parts.constructorValue, parts.phantomId))
      .map(new DynamicAgentClient(_, Some(agentId)))

  /**
   * A raw one-shot address. Final identity is supplied by invocation metadata.
   */
  def ephemeral(
    typeName: String,
    constructor: SchemaValue
  ): Either[GolemReflectError, DynamicAgentClient] =
    Transport.create(typeName, constructor, None).map(new DynamicAgentClient(_, None))
}

final class DynamicAgentMethod private[reflection] (val name: String, transport: Transport) {
  def invokeValue(input: SchemaValue): Future[Either[GolemReflectError, Invocation[SchemaValue]]] =
    transport.invokeAndAwait(name, input)
  def triggerValue(input: SchemaValue): Either[GolemReflectError, InvocationMetadata]                 = transport.trigger(name, input)
  def scheduleValue(at: Datetime, input: SchemaValue): Either[GolemReflectError, ScheduledInvocation] =
    transport.schedule(at, name, input)
}

private[reflection] final class Transport private (raw: WasmRpcApi.WasmRpcClient) {
  def invokeAndAwait(method: String, input: SchemaValue): Future[Either[GolemReflectError, Invocation[SchemaValue]]] =
    encodeAsync(input).flatMap { payload =>
      raw.asyncInvokeAndAwaitWithMetadata(method, payload) match {
        case Left(error)                => Future.successful(Left(remoteError(error)))
        case Right((metadata, pending)) =>
          FutureInterop
            .fromPromise(pending.get())
            .map { result =>
              decodeOptional(result.toOption).map(value => Invocation(toMetadata(metadata), value))
            }
            .recover {
              case js.JavaScriptException(error) => Left(remoteError(WasmRpcApi.decodeRpcError(error)))
              case NonFatal(error)               =>
                Left(GolemReflectError.Remote(AgentRpcError.Unknown("unknown", Option(error.getMessage))))
            }
      }
    }.recover { case NonFatal(error) => Left(GolemReflectError.SchemaEncode(error.getMessage)) }

  def trigger(method: String, input: SchemaValue): Either[GolemReflectError, InvocationMetadata] =
    encode(input).flatMap(payload =>
      raw
        .invokeWithMetadata(method, payload)
        .left
        .map(remoteError)
        .map(toMetadata)
    )

  def schedule(at: Datetime, method: String, input: SchemaValue): Either[GolemReflectError, ScheduledInvocation] =
    encode(input).flatMap(payload =>
      raw
        .scheduleCancelableInvocationWithMetadata(at, method, payload)
        .left
        .map(remoteError)
        .map(receipt => ScheduledInvocation(toMetadata(receipt.metadata), receipt.cancellationToken))
    )

  private def toMetadata(value: golem.runtime.rpc.InvocationMetadata): InvocationMetadata =
    InvocationMetadata(ParsedAgentId(value.agentId), value.idempotencyKey)

  private def remoteError(error: WasmRpcApi.RpcError): GolemReflectError =
    GolemReflectError.Remote(Transport.decodeRpcError(error))
}

private[reflection] object Transport {
  def create(
    typeName: String,
    constructor: SchemaValue,
    phantom: Option[Uuid]
  ): Either[GolemReflectError, Transport] =
    create(typeName, constructor, phantom, Nil)

  def create(
    typeName: String,
    constructor: SchemaValue,
    phantom: Option[Uuid],
    config: List[golem.config.ConfigOverride]
  ): Either[GolemReflectError, Transport] =
    encode(constructor).flatMap { payload =>
      try {
        val phantomArg = phantom.fold[js.UndefOr[JsSchemaUuid]](js.undefined)(uuid =>
          JsSchemaUuid(js.BigInt(uuid.highBits.toString), js.BigInt(uuid.lowBits.toString))
        )
        WasmRpcApi
          .createClient(typeName, payload, phantomArg, golem.config.ConfigOverrideEncoder.encode(config))
          .left
          .map(error => GolemReflectError.Remote(decodeRpcError(error)))
          .map(new Transport(_))
      } catch {
        case NonFatal(error) =>
          Left(GolemReflectError.Remote(AgentRpcError.Unknown("unknown", Option(error.getMessage))))
      }
    }

  private def decodeRpcError(error: WasmRpcApi.RpcError): AgentRpcError =
    error.agentError match {
      case Some(agentError) =>
        val value = agentError.asInstanceOf[js.Dynamic].selectDynamic("val")
        agentError.tag match {
          case "invalid-input"    => AgentRpcError.InvalidInput(String.valueOf(value))
          case "invalid-method"   => AgentRpcError.InvalidMethod(String.valueOf(value))
          case "invalid-type"     => AgentRpcError.InvalidType(String.valueOf(value))
          case "invalid-agent-id" => AgentRpcError.InvalidAgentId(String.valueOf(value))
          case "custom-error"     =>
            AgentRpcError.Custom(
              SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(value.asInstanceOf[JsTypedSchemaValue]))
            )
          case other => AgentRpcError.Unknown(other, None)
        }
      case None =>
        val detail = error.message.getOrElse(error.kind)
        error.kind match {
          case "protocol-error"        => AgentRpcError.Protocol(detail)
          case "denied"                => AgentRpcError.Denied(detail)
          case "not-found"             => AgentRpcError.NotFound(detail)
          case "remote-internal-error" => AgentRpcError.RemoteInternal(detail)
          case other                   => AgentRpcError.Unknown(other, error.message)
        }
    }
}

private[reflection] object ReflectionInternals {
  def validate(schema: SchemaRef, value: SchemaValue): Either[GolemReflectError, Unit] =
    schema
      .validateValue(value)
      .left
      .map(errors => GolemReflectError.Validation(errors.map(_.message).mkString("; ")))
      .map(_ => ())

  def validateInvocationOutput(
    definition: AgentMethod,
    invocation: Invocation[SchemaValue]
  ): Either[GolemReflectError, Invocation[SchemaValue]] =
    (definition.output, invocation.value) match {
      case (None, None)                => Right(invocation)
      case (Some(schema), Some(value)) => validate(schema, value).map(_ => invocation)
      case (None, Some(_))             => Left(GolemReflectError.SchemaDecode("unit method returned a value"))
      case (Some(_), None)             => Left(GolemReflectError.SchemaDecode("single-output method returned no value"))
    }

  def encode(value: SchemaValue): Either[GolemReflectError, JsSchemaValueTree] =
    try Right(SchemaWireInterop.valueTreeToJs(SchemaWire.schemaValueToWit(value)))
    catch { case NonFatal(error) => Left(GolemReflectError.SchemaEncode(error.getMessage)) }

  def encodeAsync(value: SchemaValue): Future[JsSchemaValueTree] =
    SchemaWireInterop.valueTreeToJsAsync(SchemaWire.schemaValueToWit(value))

  def decodeOptional(value: Option[JsSchemaValueTree]): Either[GolemReflectError, Option[SchemaValue]] =
    try Right(value.map(tree => SchemaWire.schemaValueFromWit(SchemaWireInterop.valueTreeFromJs(tree))))
    catch { case NonFatal(error) => Left(GolemReflectError.SchemaDecode(error.getMessage)) }

  def sequence[A](values: List[Either[GolemReflectError, A]]): Either[GolemReflectError, List[A]] =
    values.foldRight[Either[GolemReflectError, List[A]]](Right(Nil))((entry, result) =>
      entry.flatMap(value => result.map(value :: _))
    )
}
