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

import golem.host.PermissionsApi.ScopeCard
import golem.host.SchemaWireInterop
import golem.host.js.{JsAgentId, JsComponentId, JsUuid}
import golem.host.js.schema.{JsInputSchema, JsNamedField, JsOutputSchema, JsSchemaValueTree}
import golem.runtime.rpc.host.{AgentHostApi, WasmRpcApi}
import golem.runtime.rpc.{AgentRpcError, CancellationToken, CreateOptions}
import golem.runtime.tool.host.ToolHostApi
import golem.schema._
import golem.schema.SchemaTypeBody.RecordType
import golem.schema.validation.ValueValidation
import golem.schema.wire.{SchemaWire, WitSchemaValueTree}
import golem.config.{ConfigOverride, ConfigOverrideEncoder}
import golem.tool.{ToolGraphs, ToolRpcFailure}
import golem.{AgentId, Datetime, FutureInterop, InvocationId}
import zio.blocks.schema.json.Json

import java.util.UUID
import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import golem.concurrent.Implicits.global
import scala.util.{Failure, Success}
import scala.util.control.NonFatal

import ReflectionInternals._

sealed trait AgentMode extends Product with Serializable
object AgentMode {
  case object Durable   extends AgentMode
  case object Ephemeral extends AgentMode
}

final case class ComponentId(uuid: UUID) {
  private[golem] def toJs: JsComponentId =
    JsComponentId(JsUuid.fromUuid(uuid))
}

object ComponentId {
  private[golem] def fromJs(value: JsComponentId): ComponentId =
    ComponentId(JsUuid.toUuid(value.uuid))
}

sealed trait GolemReflectError extends Product with Serializable {
  def message: String
  override def toString: String = message
}

object GolemReflectError {
  final case class Discovery(message: String)     extends GolemReflectError
  final case class Identity(message: String)      extends GolemReflectError
  final case class SchemaEncode(message: String)  extends GolemReflectError
  final case class SchemaDecode(message: String)  extends GolemReflectError
  final case class Validation(message: String)    extends GolemReflectError
  final case class ToolRpc(error: ToolRpcFailure) extends GolemReflectError {
    val message: String = error.toString
  }
  final case class Remote(error: AgentRpcError) extends GolemReflectError {
    val message: String = error.message
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
  val constructorInput: SchemaRef,
  val methods: List[AgentMethod],
  val config: List[ReflectedConfigDeclaration]
) {
  val client: ReflectedAgentClientFactory = new ReflectedAgentClientFactory(this)

  def method(name: String): Option[AgentMethod] = methods.find(_.name == name)

  /**
   * A client of the existing agent of this type with the given ID. An unknown
   * ID, or the ID of an agent of another type, gives
   * [[golem.runtime.rpc.AgentRpcError.NotFound]].
   */
  def getById(agentId: AgentId): Future[Either[GolemReflectError, ReflectedAgentClient]] =
    client.getById(agentId)

  def packConfigJson(entries: List[ReflectedConfigJson]): Either[GolemReflectError, List[ConfigOverride]] =
    ReflectionInternals.sequence(entries.map { entry =>
      configDeclaration(entry.path).flatMap { declaration =>
        declaration.schema
          .validateJson(entry.value)
          .left
          .map(errors => GolemReflectError.Validation(errors.map(_.message).mkString("; ")))
          .map(value => ConfigOverride(entry.path, TypedSchemaValue(declaration.schema.graph, value)))
      }
    })

  def validateConfig(overrides: List[ConfigOverride]): Either[GolemReflectError, List[ConfigOverride]] =
    ReflectionInternals.sequence(overrides.map { entry =>
      configDeclaration(entry.path).flatMap { declaration =>
        if (!ToolGraphs.schemaShapesMatch(entry.value.graph, declaration.schema.graph))
          Left(GolemReflectError.Validation(s"Incompatible config schema at '${entry.path.mkString(".")}'"))
        else
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

  /** The deployed agent type that the type tag of `agentId` names. */
  def getAgentTypeFor(agentId: AgentId): Either[GolemReflectError, Option[AgentType]] =
    try
      AgentHostApi
        .registeredAgentTypeFor(JsAgentId.fromAgentId(agentId))
        .map(decodeAgentType)
        .fold[Either[GolemReflectError, Option[AgentType]]](Right(None))(_.map(Some(_)))
    catch { case NonFatal(error) => Left(GolemReflectError.Discovery(error.getMessage)) }

  private def decodeAgentType(registered: AgentHostApi.RegisteredAgentType): Either[GolemReflectError, AgentType] =
    try {
      val raw     = registered.agentType
      val graph   = raw.schema
      val decoded = SchemaWire.schemaGraphRootsFromWit(SchemaWireInterop.graphFromJs(graph))
      val methods = raw.methods.toList.map { method =>
        AgentMethod(
          method.name,
          method.description,
          method.promptHint.toOption,
          inputRef(decoded, method.inputSchema),
          outputRef(decoded, method.outputSchema)
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
          inputRef(decoded, raw.constructor.inputSchema),
          methods,
          raw.config.toList.map { declaration =>
            ReflectedConfigDeclaration(
              declaration.path.toList,
              declaration.source,
              SchemaRef(decoded.graph, decoded.at(declaration.valueType))
            )
          }
        )
      )
    } catch { case NonFatal(error) => Left(GolemReflectError.SchemaDecode(error.getMessage)) }

  private def inputRef(decoded: SchemaWire.DecodedSchemaGraph, input: JsInputSchema): SchemaRef = {
    if (input.tag != "parameters") throw new IllegalArgumentException(s"unknown input schema '${input.tag}'")
    val entries = input.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[js.Array[JsNamedField]].toList
    val fields  = entries.collect {
      case entry if entry.source.tag == "user-supplied" =>
        val root = decoded.at(entry.schema)
        NamedFieldType(entry.name, root, SchemaWireInterop.metadataFromJs(entry.metadata))
    }
    SchemaRef(SchemaGraph(decoded.graph.defs, SchemaType(RecordType(fields))))
  }

  private def outputRef(decoded: SchemaWire.DecodedSchemaGraph, output: JsOutputSchema): Option[SchemaRef] =
    output.tag match {
      case "unit"   => None
      case "single" =>
        val root = output.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[Int]
        Some(SchemaRef(decoded.graph, decoded.at(root)))
      case other => throw new IllegalArgumentException(s"unknown output schema '$other'")
    }
}

final class ReflectedAgentClientFactory private[reflection] (agentType: AgentType) {

  /**
   * A client of the keyed agent of the constructor parameters. A durable agent
   * is created when it is missing; every invocation of an ephemeral agent runs
   * on a new one-shot instance.
   */
  def upsert(input: Json): Either[GolemReflectError, ReflectedAgentClient] =
    pack(input).flatMap(upsertValue)

  def upsertValue(input: SchemaValue): Either[GolemReflectError, ReflectedAgentClient] =
    validate(agentType.constructorInput, input)
      .flatMap(_ => Transport.upsert(agentType.name, input))
      .map(new ReflectedAgentClient(agentType, _))

  /**
   * Creates a durable agent. With `keyed`, it is the keyed agent of the
   * constructor parameters, and an existing one gives
   * [[golem.runtime.rpc.AgentRpcError.Conflict]]; otherwise the new agent has
   * no key and a new ID.
   */
  def create(
    input: Json,
    keyed: Boolean = false,
    config: List[ReflectedConfigJson] = Nil,
    env: Map[String, String] = Map.empty
  ): Future[Either[GolemReflectError, ReflectedAgentClient]] =
    (for {
      constructor <- pack(input)
      overrides   <- agentType.packConfigJson(config)
    } yield (constructor, overrides)) match {
      case Left(error)                     => Future.successful(Left(error))
      case Right((constructor, overrides)) => createValue(constructor, keyed, overrides, env)
    }

  def createValue(
    input: SchemaValue,
    keyed: Boolean = false,
    config: List[ConfigOverride] = Nil,
    env: Map[String, String] = Map.empty
  ): Future[Either[GolemReflectError, ReflectedAgentClient]] =
    (for {
      _         <- requireDurable("create")
      _         <- validate(agentType.constructorInput, input)
      overrides <- agentType.validateConfig(config)
    } yield overrides) match {
      case Left(error)      => Future.successful(Left(error))
      case Right(overrides) =>
        Transport
          .create(agentType.name, input, CreateOptions(keyed, overrides, env))
          .map(_.map(new ReflectedAgentClient(agentType, _)))
    }

  /** See [[AgentType.getById]]. */
  def getById(agentId: AgentId): Future[Either[GolemReflectError, ReflectedAgentClient]] =
    requireDurable("getById") match {
      case Left(error) => Future.successful(Left(error))
      case Right(_)    => Transport.getById(agentType.name, agentId).map(_.map(new ReflectedAgentClient(agentType, _)))
    }

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

  /**
   * The ID of the target agent. On a client of `upsert` the host creates a
   * missing durable agent.
   */
  def agentId(): Future[Either[GolemReflectError, AgentId]] = transport.agentId()

  def method(name: String): Either[GolemReflectError, ReflectedAgentMethod] =
    agentType
      .method(name)
      .toRight(GolemReflectError.Discovery(s"Agent type '${agentType.name}' has no method '$name'"))
      .map(new ReflectedAgentMethod(_, transport))
}

final class ReflectedAgentMethod private[reflection] (val definition: AgentMethod, transport: Transport) {

  /**
   * The awaited invocation variants take an optional `scopeCard`, which the
   * call borrows; see [[golem.host.PermissionsApi.ScopeCard]].
   */
  def invoke(input: Json, scopeCard: Option[ScopeCard] = None): Future[Either[GolemReflectError, Invocation[Json]]] =
    invokeJson(input, scopeCard)

  def invokeJson(
    input: Json,
    scopeCard: Option[ScopeCard] = None
  ): Future[Either[GolemReflectError, Invocation[Json]]] =
    definition.input.packJson(input) match {
      case Left(error)  => Future.successful(Left(GolemReflectError.Validation(error.message)))
      case Right(value) =>
        invokeValue(value, scopeCard).map(_.flatMap { invocation =>
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

  def invokeValue(
    input: SchemaValue,
    scopeCard: Option[ScopeCard] = None
  ): Future[Either[GolemReflectError, Invocation[SchemaValue]]] =
    validate(definition.input, input) match {
      case Left(error) => Future.successful(Left(error))
      case Right(_)    =>
        transport
          .invokeAndAwait(definition.name, input, scopeCard)
          .map(_.flatMap(validateInvocationOutput(definition, _)))
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

final case class InvocationMetadata(agentId: AgentId, invocationId: InvocationId)
final case class Invocation[+A](metadata: InvocationMetadata, value: Option[A])
final case class ScheduledInvocation(metadata: InvocationMetadata, cancellationToken: CancellationToken)

final class DynamicAgentClient private (transport: Transport) {
  def method(name: String): DynamicAgentMethod = new DynamicAgentMethod(name, transport)

  /** The ID of the target agent; see [[ReflectedAgentClient.agentId]]. */
  def agentId(): Future[Either[GolemReflectError, AgentId]] = transport.agentId()
}

object DynamicAgentClient {

  /**
   * A client of the keyed agent of the constructor parameters. For an ephemeral
   * type, invocation metadata reports the one-shot instance that ran each call.
   */
  def upsert(typeName: String, constructor: SchemaValue): Either[GolemReflectError, DynamicAgentClient] =
    Transport.upsert(typeName, constructor).map(new DynamicAgentClient(_))

  /**
   * A client of the existing agent with the given ID. The host registry
   * resolves the agent type from the type tag of the ID.
   */
  def getById(agentId: AgentId): Future[Either[GolemReflectError, DynamicAgentClient]] =
    Transport.typeNameOf(agentId) match {
      case Left(error)     => Future.successful(Left(error))
      case Right(typeName) => Transport.getById(typeName, agentId).map(_.map(new DynamicAgentClient(_)))
    }
}

final class DynamicAgentMethod private[reflection] (val name: String, transport: Transport) {

  /**
   * `scopeCard` is borrowed by the awaited call; see
   * [[golem.host.PermissionsApi.ScopeCard]].
   */
  def invokeValue(
    input: SchemaValue,
    scopeCard: Option[ScopeCard] = None
  ): Future[Either[GolemReflectError, Invocation[SchemaValue]]] =
    transport.invokeAndAwait(name, input, scopeCard)
  def triggerValue(input: SchemaValue): Either[GolemReflectError, InvocationMetadata]                 = transport.trigger(name, input)
  def scheduleValue(at: Datetime, input: SchemaValue): Either[GolemReflectError, ScheduledInvocation] =
    transport.schedule(at, name, input)
}

private[reflection] final class Transport private (raw: WasmRpcApi.WasmRpcClient, knownAgentId: Option[AgentId]) {
  private var cachedAgentId = knownAgentId

  def agentId(): Future[Either[GolemReflectError, AgentId]] =
    cachedAgentId match {
      case Some(id) => Future.successful(Right(id))
      case None     =>
        raw
          .agentId()
          .map(_.left.map(remoteError).map { id =>
            val agentId = JsAgentId.toAgentId(id)
            cachedAgentId = Some(agentId)
            agentId
          })
    }

  def invokeAndAwait(
    method: String,
    input: SchemaValue,
    scopeCard: Option[ScopeCard]
  ): Future[Either[GolemReflectError, Invocation[SchemaValue]]] =
    if (scopeCard.exists(!_.isPresent)) Future.successful(Left(GolemReflectError.Validation(ScopeCardMoved)))
    else
      encodeWithWireAsync(input).transformWith {
        case Failure(NonFatal(error)) => Future.successful(Left(GolemReflectError.SchemaEncode(error.getMessage)))
        case Failure(error)           => Future.failed(error)
        case Success((wit, payload))  =>
          // Check again in the same turn as the host call; give the argument
          // handles back if the scope card is gone.
          if (scopeCard.exists(!_.isPresent)) {
            SchemaWireInterop.returnCapabilityHandles(wit, payload)
            Future.successful(Left(GolemReflectError.Validation(ScopeCardMoved)))
          } else
            try invokeEncoded(method, payload, scopeCard)
            catch {
              case NonFatal(error) =>
                Future.successful(
                  Left(GolemReflectError.Remote(AgentRpcError.Unknown("unknown", Option(error.getMessage))))
                )
            }
      }

  private def invokeEncoded(
    method: String,
    payload: JsSchemaValueTree,
    scopeCard: Option[ScopeCard]
  ): Future[Either[GolemReflectError, Invocation[SchemaValue]]] = {
    val call = scopeCard match {
      case Some(card) => card.borrowRaw(raw.asyncInvokeAndAwaitWithMetadata(method, payload, _))
      case None       => raw.asyncInvokeAndAwaitWithMetadata(method, payload, js.undefined)
    }
    call match {
      case Left(error)                => Future.successful(Left(remoteError(error)))
      case Right((metadata, pending)) =>
        FutureInterop
          .fromPromise(pending.get())
          .map { result =>
            metadata match {
              case Some(started) =>
                decodeOptional(result.toOption).map(value => Invocation(toMetadata(started), value))
              case None =>
                Left(GolemReflectError.Remote(AgentRpcError.Protocol("completed invocation has no metadata")))
            }
          }
          .recover {
            case js.JavaScriptException(error) => Left(remoteError(WasmRpcApi.decodeRpcError(error)))
            case NonFatal(error)               =>
              Left(GolemReflectError.Remote(AgentRpcError.Unknown("unknown", Option(error.getMessage))))
          }
    }
  }

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
    InvocationMetadata(value.agentId, value.invocationId)

  private def remoteError(error: WasmRpcApi.RpcError): GolemReflectError =
    GolemReflectError.Remote(AgentRpcError.fromRpcError(error))
}

private[reflection] object Transport {
  def upsert(typeName: String, constructor: SchemaValue): Either[GolemReflectError, Transport] =
    encode(constructor).flatMap { payload =>
      guarded(
        WasmRpcApi
          .upsert(typeName, payload)
          .left
          .map(error => GolemReflectError.Remote(AgentRpcError.fromRpcError(error)))
          .map(new Transport(_, None))
      )
    }

  def create(
    typeName: String,
    constructor: SchemaValue,
    options: CreateOptions
  ): Future[Either[GolemReflectError, Transport]] =
    encode(constructor) match {
      case Left(error)    => Future.successful(Left(error))
      case Right(payload) =>
        guardedFuture(
          WasmRpcApi
            .create(typeName, payload, options.keyed, ConfigOverrideEncoder.encode(options.config), options.env)
            .map(
              _.left
                .map(error => GolemReflectError.Remote(AgentRpcError.fromRpcError(error)))
                .map { case (id, rpc) => new Transport(rpc, Some(JsAgentId.toAgentId(id))) }
            )
        )
    }

  def getById(typeName: String, agentId: AgentId): Future[Either[GolemReflectError, Transport]] =
    guardedFuture(
      Future.successful(
        WasmRpcApi
          .getById(typeName, JsAgentId.fromAgentId(agentId))
          .left
          .map(error => GolemReflectError.Remote(AgentRpcError.fromRpcError(error)))
          .map(new Transport(_, Some(agentId)))
      )
    )

  /**
   * The name of the deployed agent type that the type tag of `agentId` names.
   */
  def typeNameOf(agentId: AgentId): Either[GolemReflectError, String] =
    guarded(
      AgentHostApi
        .registeredAgentTypeFor(JsAgentId.fromAgentId(agentId))
        .map(_.agentType.typeName)
        .toRight(GolemReflectError.Remote(AgentRpcError.NotFound(s"No deployed agent type has the ID $agentId")))
    )

  private def guarded[A](thunk: => Either[GolemReflectError, A]): Either[GolemReflectError, A] =
    try thunk
    catch {
      case NonFatal(error) =>
        Left(GolemReflectError.Remote(AgentRpcError.Unknown("unknown", Option(error.getMessage))))
    }

  private def guardedFuture[A](thunk: => Future[Either[GolemReflectError, A]]): Future[Either[GolemReflectError, A]] =
    (try thunk
    catch { case NonFatal(error) => Future.failed(error) }).recover { case NonFatal(error) =>
      Left(GolemReflectError.Remote(AgentRpcError.Unknown("unknown", Option(error.getMessage))))
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

  def encodeWithWireAsync(value: SchemaValue): Future[(WitSchemaValueTree, JsSchemaValueTree)] =
    try {
      val wit = SchemaWire.schemaValueToWit(value)
      SchemaWireInterop.valueTreeToJsAsync(wit).map(lowered => (wit, lowered))
    } catch { case NonFatal(error) => Future.failed(error) }

  val ScopeCardMoved: String =
    "the scope card has been released or moved; derive a new scope card in this invocation"

  def decodeOptional(value: Option[JsSchemaValueTree]): Either[GolemReflectError, Option[SchemaValue]] =
    try Right(value.map(tree => SchemaWire.schemaValueFromWit(SchemaWireInterop.valueTreeFromJs(tree))))
    catch { case NonFatal(error) => Left(GolemReflectError.SchemaDecode(error.getMessage)) }

  def sequence[A](values: List[Either[GolemReflectError, A]]): Either[GolemReflectError, List[A]] =
    values.foldRight[Either[GolemReflectError, List[A]]](Right(Nil))((entry, result) =>
      entry.flatMap(value => result.map(value :: _))
    )
}
