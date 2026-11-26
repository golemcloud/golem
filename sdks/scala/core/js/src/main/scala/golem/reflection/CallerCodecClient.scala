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

import golem.config.ConfigOverride
import golem.runtime.{InputRecordCodec, OutputCodec, OutputMetadata}
import golem.runtime.rpc.{CreateOptions, Invocation}
import golem.schema.SchemaValue
import golem.{AgentId, Datetime}

import scala.concurrent.Future
import golem.concurrent.Implicits.global
import scala.annotation.targetName
import scala.util.control.NonFatal

sealed trait AgentClientCapability
sealed trait MethodOnly    extends AgentClientCapability
sealed trait Full          extends AgentClientCapability
sealed trait DurableFull   extends Full
sealed trait EphemeralFull extends Full
sealed trait NoConfig

sealed trait CanBindAgentClient[Capability <: AgentClientCapability]

object CanBindAgentClient {
  implicit object MethodOnlyCanBind  extends CanBindAgentClient[MethodOnly]
  implicit object DurableFullCanBind extends CanBindAgentClient[DurableFull]
}

trait AgentConfigCodec[Config] {
  def overrides(config: Config): List[ConfigOverride]
}

object AgentConfigCodec {
  def apply[Config](encode: Config => List[ConfigOverride]): AgentConfigCodec[Config] =
    new AgentConfigCodec[Config] {
      def overrides(config: Config): List[ConfigOverride] = encode(config)
    }
}

/** A discovery-free, caller-owned static agent client definition. */
final class AgentClientDefinition[Capability <: AgentClientCapability, Constructor, Config] private (
  private[reflection] val contractName: Option[String],
  private[reflection] val contractMode: Option[AgentMode],
  private[reflection] val constructorCodec: Option[InputRecordCodec[Constructor]],
  private[reflection] val configCodec: Option[AgentConfigCodec[Config]]
) {
  def method[Input, Output](
    name: String,
    input: InputRecordCodec[Input],
    output: OutputCodec[Output]
  ): CallerCodecMethod[Input, Output] =
    CallerCodecMethod(name, input, output)

  def client(implicit full: Capability <:< Full): CallerCodecClientFactory[Constructor, Config] =
    new CallerCodecClientFactory(this.asInstanceOf[AgentClientDefinition[Full, Constructor, Config]])

  /**
   * A client of the existing agent with the given ID. A definition with a
   * contract name binds that agent type; a method-only definition binds the
   * agent type that the type tag of the ID names. An unknown ID, or the ID of
   * an agent of another type, gives
   * [[golem.runtime.rpc.AgentRpcError.NotFound]].
   */
  def getById(agentId: AgentId)(implicit
    canBind: CanBindAgentClient[Capability]
  ): Either[GolemReflectError, CallerCodecAgentClient] =
    contractName
      .map(Right(_))
      .getOrElse(Transport.typeNameOf(agentId))
      .flatMap(Transport.getById(_, agentId))
      .map(new CallerCodecAgentClient(_))
}

object AgentClientDefinition {
  def methodOnly: AgentClientDefinition[MethodOnly, Unit, NoConfig] =
    new AgentClientDefinition(None, None, None, None)

  def full[Constructor](
    name: String,
    constructor: InputRecordCodec[Constructor]
  ): AgentClientDefinition[DurableFull, Constructor, NoConfig] =
    new AgentClientDefinition(Some(name), Some(AgentMode.Durable), Some(constructor), None)

  @targetName("completeDurable")
  def full[Constructor](
    name: String,
    mode: AgentMode.Durable.type,
    constructor: InputRecordCodec[Constructor]
  ): AgentClientDefinition[DurableFull, Constructor, NoConfig] =
    new AgentClientDefinition(Some(name), Some(mode), Some(constructor), None)

  @targetName("completeEphemeral")
  def full[Constructor](
    name: String,
    mode: AgentMode.Ephemeral.type,
    constructor: InputRecordCodec[Constructor]
  ): AgentClientDefinition[EphemeralFull, Constructor, NoConfig] =
    new AgentClientDefinition(Some(name), Some(mode), Some(constructor), None)

  @targetName("completeDurableWithConfig")
  def full[Constructor, Config](
    name: String,
    mode: AgentMode.Durable.type,
    constructor: InputRecordCodec[Constructor],
    config: AgentConfigCodec[Config]
  ): AgentClientDefinition[DurableFull, Constructor, Config] =
    new AgentClientDefinition(Some(name), Some(mode), Some(constructor), Some(config))

  @targetName("completeEphemeralWithConfig")
  def full[Constructor, Config](
    name: String,
    mode: AgentMode.Ephemeral.type,
    constructor: InputRecordCodec[Constructor],
    config: AgentConfigCodec[Config]
  ): AgentClientDefinition[EphemeralFull, Constructor, Config] =
    new AgentClientDefinition(Some(name), Some(mode), Some(constructor), Some(config))
}

final case class CallerCodecMethod[Input, Output](
  name: String,
  input: InputRecordCodec[Input],
  output: OutputCodec[Output]
)

final class CallerCodecClientFactory[Constructor, Config] private[reflection] (
  definition: AgentClientDefinition[Full, Constructor, Config]
) {

  /**
   * A client of the keyed agent of the constructor parameters. A durable agent
   * is created when it is missing; every invocation of an ephemeral agent runs
   * on a new one-shot instance.
   */
  def upsert(input: Constructor): Either[GolemReflectError, CallerCodecAgentClient] =
    encodeConstructor(input)
      .flatMap(Transport.upsert(definition.contractName.get, _))
      .map(new CallerCodecAgentClient(_))

  /**
   * Creates a durable agent. With `keyed`, it is the keyed agent of the
   * constructor parameters, and an existing one gives
   * [[golem.runtime.rpc.AgentRpcError.Conflict]]; otherwise the new agent has
   * no key and a new ID. `env` is added to the environment the manifest
   * defines.
   */
  def create(
    input: Constructor,
    keyed: Boolean = false,
    env: Map[String, String] = Map.empty
  ): Future[Either[GolemReflectError, CallerCodecAgentClient]] =
    createWithOverrides(input, CreateOptions(keyed, Nil, env))

  /** Like the other `create`, with `config` overriding local configuration. */
  def create(
    input: Constructor,
    config: Config,
    keyed: Boolean,
    env: Map[String, String]
  ): Future[Either[GolemReflectError, CallerCodecAgentClient]] =
    encodeConfig(config) match {
      case Left(error)      => Future.successful(Left(error))
      case Right(overrides) => createWithOverrides(input, CreateOptions(keyed, overrides, env))
    }

  /** See [[AgentClientDefinition.getById]]. */
  def getById(agentId: AgentId): Either[GolemReflectError, CallerCodecAgentClient] =
    requireDurable("getById")
      .flatMap(_ => Transport.getById(definition.contractName.get, agentId))
      .map(new CallerCodecAgentClient(_))

  private def createWithOverrides(
    input: Constructor,
    options: CreateOptions
  ): Future[Either[GolemReflectError, CallerCodecAgentClient]] =
    requireDurable("create").flatMap(_ => encodeConstructor(input)) match {
      case Left(error)        => Future.successful(Left(error))
      case Right(constructor) =>
        Transport
          .create(definition.contractName.get, constructor, options)
          .map(_.map(new CallerCodecAgentClient(_)))
    }

  private def encodeConstructor(input: Constructor): Either[GolemReflectError, SchemaValue] =
    try Right(definition.constructorCodec.get.toValue(input))
    catch { case NonFatal(error) => Left(GolemReflectError.SchemaEncode(error.getMessage)) }

  private def requireDurable(operation: String): Either[GolemReflectError, Unit] =
    Either.cond(
      definition.contractMode.contains(AgentMode.Durable),
      (),
      GolemReflectError.Identity(s"$operation is not available for ephemeral agent types")
    )

  private def encodeConfig(config: Config): Either[GolemReflectError, List[ConfigOverride]] =
    try Right(definition.configCodec.fold(List.empty[ConfigOverride])(_.overrides(config)))
    catch { case NonFatal(error) => Left(GolemReflectError.SchemaEncode(error.getMessage)) }
}

final class CallerCodecAgentClient private[reflection] (transport: Transport) {

  /** The ID of the target agent; see [[ReflectedAgentClient.agentId]]. */
  def agentId(): Future[Either[GolemReflectError, AgentId]] = transport.agentId()

  def method[Input, Output](definition: CallerCodecMethod[Input, Output]): CallerCodecBoundMethod[Input, Output] =
    new CallerCodecBoundMethod(definition, transport)
}

final class CallerCodecBoundMethod[Input, Output] private[reflection] (
  definition: CallerCodecMethod[Input, Output],
  transport: Transport
) {

  /**
   * `scopeCard` is borrowed by the awaited call; see
   * [[golem.host.PermissionsApi.ScopeCard]].
   */
  def invoke(
    input: Input,
    scopeCard: Option[golem.host.PermissionsApi.ScopeCard] = None
  ): Future[Either[GolemReflectError, TypedInvocation[Output]]] =
    encodeInput(input) match {
      case Left(error)  => Future.successful(Left(error))
      case Right(value) =>
        transport
          .invokeAndAwait(definition.name, value, scopeCard)
          .map(_.flatMap { invocation =>
            decodeOutput(invocation.value).map(output => TypedInvocation(invocation.metadata, output))
          })
    }

  /**
   * Triggers the method, and returns the invocation's handle once the target
   * has durably accepted it. Never lends a scope card, and rejects streaming
   * methods. See [[golem.runtime.rpc.Invocation]].
   */
  def trigger(input: Input): Future[Either[GolemReflectError, Invocation[Output]]] =
    started("trigger", input)(transport.trigger(definition.name, _))

  /**
   * Schedules the method for `at`, and returns the invocation's handle once the
   * target has durably accepted it. Never lends a scope card, and rejects
   * streaming methods. See [[golem.runtime.rpc.Invocation]].
   */
  def schedule(at: Datetime, input: Input): Future[Either[GolemReflectError, Invocation[Output]]] =
    started("schedule", input)(transport.schedule(at, definition.name, _))

  private def started(operation: String, input: Input)(
    start: SchemaValue => Future[Either[GolemReflectError, Invocation[Option[SchemaValue]]]]
  ): Future[Either[GolemReflectError, Invocation[Output]]] =
    rejectNonAwaitedStreams(operation).flatMap(_ => encodeInput(input)) match {
      case Left(error)  => Future.successful(Left(error))
      case Right(value) =>
        start(value).map(_.map(_.mapResult { output =>
          decodeOutput(output).fold(error => throw new IllegalArgumentException(error.message), identity)
        }))
    }

  private def encodeInput(input: Input): Either[GolemReflectError, SchemaValue] =
    try Right(definition.input.toValue(input))
    catch { case NonFatal(error) => Left(GolemReflectError.SchemaEncode(error.getMessage)) }

  private def decodeOutput(value: Option[SchemaValue]): Either[GolemReflectError, Output] =
    definition.output.metadata match {
      case OutputMetadata.Unit =>
        Either.cond(
          value.isEmpty,
          ().asInstanceOf[Output],
          GolemReflectError.SchemaDecode("unit method returned a value")
        )
      case OutputMetadata.Single(_) =>
        value
          .toRight(GolemReflectError.SchemaDecode("single-output method returned no value"))
          .flatMap(schemaValue =>
            definition.output.from.get
              .fromValue(schemaValue)
              .left
              .map(error => GolemReflectError.SchemaDecode(error.message))
          )
    }

  private def rejectNonAwaitedStreams(operation: String): Either[GolemReflectError, Unit] = {
    val outputContainsStream = definition.output.metadata match {
      case OutputMetadata.Unit          => false
      case OutputMetadata.Single(graph) => graph.containsStream
    }
    Either.cond(
      !definition.input.graph.containsStream && !outputContainsStream,
      (),
      GolemReflectError.Validation(s"$operation is unavailable for streaming method '${definition.name}'")
    )
  }
}

/**
 * The result of an awaited typed invocation together with its final identity.
 */
final case class TypedInvocation[+A](metadata: InvocationMetadata, value: A)
