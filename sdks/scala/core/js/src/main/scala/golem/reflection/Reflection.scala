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
  JsUuid => JsSchemaUuid
}
import golem.runtime.rpc.host.{AgentHostApi, WasmRpcApi}
import golem.runtime.rpc.{CancellationToken, InvocationReceipt}
import golem.schema._
import golem.schema.SchemaTypeBody.RecordType
import golem.schema.validation.ValueValidation
import golem.schema.wire.SchemaWire
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

final case class AgentId(componentId: ComponentId, value: String) {
  def parts: Either[GolemReflectError, AgentIdParts]               = AgentId.parse(this)
  def dynamicClient: Either[GolemReflectError, DynamicAgentClient] = DynamicAgentClient.fromAgentId(this)
  def client[Constructor](
    definition: AgentClientDefinition[Constructor]
  ): Either[GolemReflectError, CallerCodecAgentClient[Constructor]] = definition.bind(this)
}

final case class AgentIdParts(typeName: String, constructorValue: SchemaValue, phantomId: Option[Uuid])

object AgentId {
  def create(
    componentId: ComponentId,
    typeName: String,
    constructorValue: SchemaValue,
    phantomId: Option[Uuid] = None
  ): Either[GolemReflectError, AgentId] =
    encode(constructorValue).flatMap(payload =>
      AgentHostApi
        .makeAgentId(typeName, payload, phantomId)
        .left
        .map(GolemReflectError.Identity.apply)
        .map(AgentId(componentId, _))
    )

  def parse(agentId: AgentId): Either[GolemReflectError, AgentIdParts] =
    AgentHostApi
      .parseAgentId(agentId.value)
      .left
      .map(GolemReflectError.Identity.apply)
      .flatMap { parts =>
        try
          Right(
            AgentIdParts(
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
  final case class Remote(message: String)       extends GolemReflectError
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
  val methods: List[AgentMethod]
) {
  val client: ReflectedAgentClientFactory = new ReflectedAgentClientFactory(this)

  def method(name: String): Option[AgentMethod] = methods.find(_.name == name)

  def agentId(input: Json, phantomId: Option[Uuid] = None): Either[GolemReflectError, AgentId] =
    constructorInput
      .packJson(input)
      .left
      .map(error => GolemReflectError.Validation(error.message))
      .flatMap(agentIdValue(_, phantomId))

  def agentIdValue(input: SchemaValue, phantomId: Option[Uuid] = None): Either[GolemReflectError, AgentId] =
    validate(constructorInput, input).flatMap(_ => AgentId.create(implementedBy, name, input, phantomId))

  def bind(agentId: AgentId): Either[GolemReflectError, ReflectedAgentClient] =
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
      client <- client.createValue(parts.constructorValue, parts.phantomId)
    } yield client
}

object Reflection {
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

  private[reflection] def componentIdFor(name: String): Either[GolemReflectError, ComponentId] =
    try
      AgentHostApi
        .registeredAgentType(name)
        .map(value => ComponentId.fromJs(value.implementedBy))
        .toRight(GolemReflectError.Discovery(s"Agent type '$name' is not registered in the current environment"))
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
          methods
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

final case class ReflectedPhantomClient(agentId: AgentId, phantomId: Uuid, client: ReflectedAgentClient)

final class ReflectedAgentClientFactory private[reflection] (agentType: AgentType) {
  def get(input: Json): Either[GolemReflectError, ReflectedAgentClient] =
    requireDurable("get").flatMap(_ => pack(input)).flatMap(createValue(_, None))

  def getValue(input: SchemaValue): Either[GolemReflectError, ReflectedAgentClient] =
    requireDurable("getValue").flatMap(_ => createValue(input, None))

  def getPhantom(input: Json, phantomId: Uuid): Either[GolemReflectError, ReflectedAgentClient] =
    pack(input).flatMap(createValue(_, Some(phantomId)))

  def getPhantomValue(input: SchemaValue, phantomId: Uuid): Either[GolemReflectError, ReflectedAgentClient] =
    createValue(input, Some(phantomId))

  def newPhantom(input: Json): Either[GolemReflectError, Either[ReflectedAgentClient, ReflectedPhantomClient]] =
    pack(input).flatMap(newPhantomValue)

  def newPhantomValue(
    input: SchemaValue
  ): Either[GolemReflectError, Either[ReflectedAgentClient, ReflectedPhantomClient]] =
    if (agentType.mode == AgentMode.Ephemeral) createValue(input, None).map(Left(_))
    else {
      val phantom = Uuid.random()
      for {
        id     <- agentType.agentIdValue(input, Some(phantom))
        client <- createValue(input, Some(phantom))
      } yield Right(ReflectedPhantomClient(id, phantom, client))
    }

  private[reflection] def createValue(
    input: SchemaValue,
    phantomId: Option[Uuid]
  ): Either[GolemReflectError, ReflectedAgentClient] =
    validate(agentType.constructorInput, input)
      .flatMap(_ => Transport.create(agentType.implementedBy, agentType.name, input, phantomId))
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

final case class InvocationMetadata(agentId: AgentId, idempotencyKey: String)
final case class Invocation[+A](metadata: InvocationMetadata, value: Option[A])
final case class ScheduledInvocation(metadata: InvocationMetadata, cancellationToken: CancellationToken)

final class DynamicAgentClient private (transport: Transport, val agentId: Option[AgentId]) {
  def method(name: String): DynamicAgentMethod = new DynamicAgentMethod(name, transport)
}

object DynamicAgentClient {
  def fromAgentId(agentId: AgentId): Either[GolemReflectError, DynamicAgentClient] =
    agentId.parts
      .flatMap(parts => Transport.create(agentId.componentId, parts.typeName, parts.constructorValue, parts.phantomId))
      .map(new DynamicAgentClient(_, Some(agentId)))

  /**
   * A raw one-shot address. Final identity is supplied by invocation metadata.
   */
  def ephemeral(
    componentId: ComponentId,
    typeName: String,
    constructor: SchemaValue
  ): Either[GolemReflectError, DynamicAgentClient] =
    Transport.create(componentId, typeName, constructor, None).map(new DynamicAgentClient(_, None))
}

final class DynamicAgentMethod private[reflection] (val name: String, transport: Transport) {
  def invokeValue(input: SchemaValue): Future[Either[GolemReflectError, Invocation[SchemaValue]]] =
    transport.invokeAndAwait(name, input)
  def triggerValue(input: SchemaValue): Either[GolemReflectError, InvocationMetadata]                 = transport.trigger(name, input)
  def scheduleValue(at: Datetime, input: SchemaValue): Either[GolemReflectError, ScheduledInvocation] =
    transport.schedule(at, name, input)
}

private[reflection] final class Transport private (componentId: ComponentId, raw: WasmRpcApi.WasmRpcClient) {
  def invokeAndAwait(method: String, input: SchemaValue): Future[Either[GolemReflectError, Invocation[SchemaValue]]] =
    encodeAsync(input).flatMap { payload =>
      raw.asyncInvokeAndAwaitWithMetadata(method, payload) match {
        case Left(error)                => Future.successful(Left(GolemReflectError.Remote(error.toString)))
        case Right((metadata, pending)) =>
          FutureInterop
            .fromPromise(pending.get())
            .map { result =>
              decodeOptional(result.toOption).map(value => Invocation(toMetadata(metadata), value))
            }
            .recover { case NonFatal(error) => Left(GolemReflectError.Remote(error.getMessage)) }
      }
    }.recover { case NonFatal(error) => Left(GolemReflectError.SchemaEncode(error.getMessage)) }

  def trigger(method: String, input: SchemaValue): Either[GolemReflectError, InvocationMetadata] =
    encode(input).flatMap(payload =>
      raw
        .invokeWithMetadata(method, payload)
        .left
        .map(error => GolemReflectError.Remote(error.toString))
        .map(toMetadata)
    )

  def schedule(at: Datetime, method: String, input: SchemaValue): Either[GolemReflectError, ScheduledInvocation] =
    encode(input).flatMap(payload =>
      raw
        .scheduleCancelableInvocationWithMetadata(at, method, payload)
        .left
        .map(error => GolemReflectError.Remote(error.toString))
        .map(receipt => ScheduledInvocation(toMetadata(receipt.metadata), receipt.cancellationToken))
    )

  private def toMetadata(value: golem.runtime.rpc.InvocationMetadata): InvocationMetadata =
    InvocationMetadata(AgentId(componentId, value.agentId), value.idempotencyKey)
}

private[reflection] object Transport {
  def create(
    componentId: ComponentId,
    typeName: String,
    constructor: SchemaValue,
    phantom: Option[Uuid]
  ): Either[GolemReflectError, Transport] =
    encode(constructor).map { payload =>
      val phantomArg = phantom.fold[js.UndefOr[JsSchemaUuid]](js.undefined)(uuid =>
        JsSchemaUuid(js.BigInt(uuid.highBits.toString), js.BigInt(uuid.lowBits.toString))
      )
      new Transport(componentId, WasmRpcApi.newClient(typeName, payload, phantomArg, js.Array()))
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
