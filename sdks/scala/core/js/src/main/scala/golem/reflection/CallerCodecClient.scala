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

import golem.config.{ConfigOverride, ConfigOverrideEncoder}
import golem.runtime.{InputRecordCodec, OutputCodec, OutputMetadata}
import golem.schema.SchemaValue
import golem.{Datetime, Uuid}

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.util.control.NonFatal

sealed trait AgentClientCapability
sealed trait BindingOnly extends AgentClientCapability
sealed trait Complete    extends AgentClientCapability
sealed trait NoConfig

trait AgentConfigCodec[Config] {
  def overrides(config: Config): List[ConfigOverride]
}

object AgentConfigCodec {
  def apply[Config](encode: Config => List[ConfigOverride]): AgentConfigCodec[Config] =
    new AgentConfigCodec[Config] {
      def overrides(config: Config): List[ConfigOverride] = encode(config)
    }
}

/** A discovery-free, caller-authored typed agent contract. */
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

  def client(implicit complete: Capability =:= Complete): CallerCodecClientFactory[Constructor, Config] =
    new CallerCodecClientFactory(this.asInstanceOf[AgentClientDefinition[Complete, Constructor, Config]])

  def agentId(
    input: Constructor,
    phantomId: Option[Uuid] = None
  )(implicit complete: Capability =:= Complete): Either[GolemReflectError, ParsedAgentId] =
    try ParsedAgentId.create(contractName.get, constructorCodec.get.toValue(input), phantomId)
    catch { case NonFatal(error) => Left(GolemReflectError.SchemaEncode(error.getMessage)) }

  def bind(agentId: ParsedAgentId): Either[GolemReflectError, CallerCodecAgentClient] =
    for {
      parts <- agentId.parts
      _     <- contractName match {
             case None       => Right(())
             case Some(name) =>
               Either.cond(
                 parts.typeName == name,
                 (),
                 GolemReflectError.Identity(s"Agent client contract '$name' cannot bind '${parts.typeName}'")
               )
           }
      _ <- contractMode match {
             case Some(AgentMode.Ephemeral) =>
               Left(
                 GolemReflectError.Identity(
                   s"Cannot bind an existing identity to ephemeral agent type '${contractName.get}'"
                 )
               )
             case _ => Right(())
           }
      _ <- constructorCodec match {
             case None        => Right(())
             case Some(codec) => ReflectionInternals.validate(SchemaRef(codec.graph), parts.constructorValue)
           }
      transport <- Transport.create(parts.typeName, parts.constructorValue, parts.phantomId)
    } yield new CallerCodecAgentClient(transport)
}

object AgentClientDefinition {
  def bindingOnly: AgentClientDefinition[BindingOnly, Unit, NoConfig] =
    new AgentClientDefinition(None, None, None, None)

  def complete[Constructor](
    name: String,
    constructor: InputRecordCodec[Constructor],
    mode: AgentMode = AgentMode.Durable
  ): AgentClientDefinition[Complete, Constructor, NoConfig] =
    new AgentClientDefinition(Some(name), Some(mode), Some(constructor), None)

  def complete[Constructor, Config](
    name: String,
    mode: AgentMode,
    constructor: InputRecordCodec[Constructor],
    config: AgentConfigCodec[Config]
  ): AgentClientDefinition[Complete, Constructor, Config] =
    new AgentClientDefinition(Some(name), Some(mode), Some(constructor), Some(config))
}

final case class CallerCodecMethod[Input, Output](
  name: String,
  input: InputRecordCodec[Input],
  output: OutputCodec[Output]
)

final case class CallerCodecPhantomClient(
  agentId: ParsedAgentId,
  phantomId: Uuid,
  client: CallerCodecAgentClient
)

final class CallerCodecClientFactory[Constructor, Config] private[reflection] (
  definition: AgentClientDefinition[Complete, Constructor, Config]
) {
  def get(input: Constructor)(implicit
    noConfig: Config =:= NoConfig
  ): Either[GolemReflectError, CallerCodecAgentClient] =
    getWithOverrides(input, Nil)

  def get(input: Constructor, config: Config): Either[GolemReflectError, CallerCodecAgentClient] =
    getWithOverrides(input, encodeConfig(config))

  def getPhantom(input: Constructor, phantomId: Uuid)(implicit
    noConfig: Config =:= NoConfig
  ): Either[GolemReflectError, CallerCodecAgentClient] =
    create(input, Some(phantomId), Nil)

  def getPhantom(
    input: Constructor,
    phantomId: Uuid,
    config: Config
  ): Either[GolemReflectError, CallerCodecAgentClient] =
    create(input, Some(phantomId), encodeConfig(config))

  def newPhantom(
    input: Constructor
  )(implicit
    noConfig: Config =:= NoConfig
  ): Either[GolemReflectError, Either[CallerCodecAgentClient, CallerCodecPhantomClient]] =
    newPhantomWithOverrides(input, Nil)

  def newPhantom(
    input: Constructor,
    config: Config
  ): Either[GolemReflectError, Either[CallerCodecAgentClient, CallerCodecPhantomClient]] =
    newPhantomWithOverrides(input, encodeConfig(config))

  private def newPhantomWithOverrides(
    input: Constructor,
    overrides: List[ConfigOverride]
  ): Either[GolemReflectError, Either[CallerCodecAgentClient, CallerCodecPhantomClient]] =
    if (definition.contractMode.contains(AgentMode.Ephemeral)) create(input, None, overrides).map(Left(_))
    else {
      val phantom = Uuid.random()
      for {
        constructor <- encodeConstructor(input)
        id          <- ParsedAgentId.create(definition.contractName.get, constructor, Some(phantom))
        transport   <- Transport.create(definition.contractName.get, constructor, Some(phantom), overrides)
        client       = new CallerCodecAgentClient(transport)
      } yield Right(CallerCodecPhantomClient(id, phantom, client))
    }

  private def getWithOverrides(input: Constructor, overrides: List[ConfigOverride]) =
    requireDurable("get").flatMap(_ => create(input, None, overrides))

  private def create(
    input: Constructor,
    phantomId: Option[Uuid],
    overrides: List[ConfigOverride]
  ): Either[GolemReflectError, CallerCodecAgentClient] =
    encodeConstructor(input).flatMap(createValue(_, phantomId, overrides))

  private def createValue(
    constructor: SchemaValue,
    phantomId: Option[Uuid],
    overrides: List[ConfigOverride]
  ): Either[GolemReflectError, CallerCodecAgentClient] =
    Transport
      .create(definition.contractName.get, constructor, phantomId, overrides)
      .map(new CallerCodecAgentClient(_))

  private def encodeConstructor(input: Constructor): Either[GolemReflectError, SchemaValue] =
    try Right(definition.constructorCodec.get.toValue(input))
    catch { case NonFatal(error) => Left(GolemReflectError.SchemaEncode(error.getMessage)) }

  private def requireDurable(operation: String): Either[GolemReflectError, Unit] =
    Either.cond(
      definition.contractMode.contains(AgentMode.Durable),
      (),
      GolemReflectError.Identity(s"$operation is not available for ephemeral agent types")
    )

  private def encodeConfig(config: Config): List[ConfigOverride] =
    definition.configCodec.fold(List.empty[ConfigOverride])(_.overrides(config))
}

final class CallerCodecAgentClient private[reflection] (transport: Transport) {
  def method[Input, Output](definition: CallerCodecMethod[Input, Output]): CallerCodecBoundMethod[Input, Output] =
    new CallerCodecBoundMethod(definition, transport)
}

final class CallerCodecBoundMethod[Input, Output] private[reflection] (
  definition: CallerCodecMethod[Input, Output],
  transport: Transport
) {
  def invoke(input: Input): Future[Either[GolemReflectError, TypedInvocation[Output]]] =
    encodeInput(input) match {
      case Left(error)  => Future.successful(Left(error))
      case Right(value) =>
        transport
          .invokeAndAwait(definition.name, value)
          .map(_.flatMap { invocation =>
            decodeOutput(invocation.value).map(output => TypedInvocation(invocation.metadata, output))
          })
    }

  def trigger(input: Input): Either[GolemReflectError, InvocationMetadata] =
    rejectNonAwaitedStreams("trigger").flatMap(_ => encodeInput(input)).flatMap(transport.trigger(definition.name, _))

  def schedule(at: Datetime, input: Input): Either[GolemReflectError, ScheduledInvocation] =
    rejectNonAwaitedStreams("schedule")
      .flatMap(_ => encodeInput(input))
      .flatMap(transport.schedule(at, definition.name, _))

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

final case class TypedInvocation[+A](metadata: InvocationMetadata, value: A)
