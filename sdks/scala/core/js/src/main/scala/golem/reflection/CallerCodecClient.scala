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

import golem.runtime.{InputRecordCodec, OutputCodec, OutputMetadata}
import golem.schema.SchemaValue
import golem.{Datetime, Uuid}

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.util.control.NonFatal

/** A discovery-free, caller-authored typed agent contract. */
final class AgentClientDefinition[Constructor] private (
  val name: String,
  val mode: AgentMode,
  val constructor: InputRecordCodec[Constructor]
) {
  val client: CallerCodecClientFactory[Constructor] = new CallerCodecClientFactory(this)

  def method[Input, Output](
    name: String,
    input: InputRecordCodec[Input],
    output: OutputCodec[Output]
  ): CallerCodecMethod[Input, Output] =
    CallerCodecMethod(name, input, output)

  def bind(agentId: AgentId): Either[GolemReflectError, CallerCodecAgentClient[Constructor]] =
    for {
      parts <- agentId.parts
      _     <- Either.cond(
             parts.typeName == name,
             (),
             GolemReflectError.Identity(s"Agent client contract '$name' cannot bind '${parts.typeName}'")
           )
      _ <- Either.cond(
             mode == AgentMode.Durable,
             (),
             GolemReflectError.Identity(s"Cannot bind an existing identity to ephemeral agent type '$name'")
           )
      componentId <- Reflection.componentIdFor(name)
      transport   <- Transport.create(componentId, name, parts.constructorValue, parts.phantomId)
    } yield new CallerCodecAgentClient(this, transport)
}

object AgentClientDefinition {
  def apply[Constructor](
    name: String,
    constructor: InputRecordCodec[Constructor],
    mode: AgentMode = AgentMode.Durable
  ): AgentClientDefinition[Constructor] =
    new AgentClientDefinition(name, mode, constructor)
}

final case class CallerCodecMethod[Input, Output](
  name: String,
  input: InputRecordCodec[Input],
  output: OutputCodec[Output]
)

final case class CallerCodecPhantomClient[Constructor](
  agentId: AgentId,
  phantomId: Uuid,
  client: CallerCodecAgentClient[Constructor]
)

final class CallerCodecClientFactory[Constructor] private[reflection] (
  definition: AgentClientDefinition[Constructor]
) {
  def get(input: Constructor): Either[GolemReflectError, CallerCodecAgentClient[Constructor]] =
    requireDurable("get").flatMap(_ => create(input, None))

  def getPhantom(input: Constructor, phantomId: Uuid): Either[GolemReflectError, CallerCodecAgentClient[Constructor]] =
    create(input, Some(phantomId))

  def newPhantom(
    input: Constructor
  ): Either[GolemReflectError, Either[CallerCodecAgentClient[Constructor], CallerCodecPhantomClient[Constructor]]] =
    if (definition.mode == AgentMode.Ephemeral) create(input, None).map(Left(_))
    else {
      val phantom = Uuid.random()
      for {
        constructor <- encodeConstructor(input)
        componentId <- Reflection.componentIdFor(definition.name)
        id          <- AgentId.create(componentId, definition.name, constructor, Some(phantom))
        transport   <- Transport.create(componentId, definition.name, constructor, Some(phantom))
        client       = new CallerCodecAgentClient(definition, transport)
      } yield Right(CallerCodecPhantomClient(id, phantom, client))
    }

  private def create(
    input: Constructor,
    phantomId: Option[Uuid]
  ): Either[GolemReflectError, CallerCodecAgentClient[Constructor]] =
    encodeConstructor(input).flatMap(createValue(_, phantomId))

  private def createValue(
    constructor: SchemaValue,
    phantomId: Option[Uuid]
  ): Either[GolemReflectError, CallerCodecAgentClient[Constructor]] =
    Reflection
      .componentIdFor(definition.name)
      .flatMap(Transport.create(_, definition.name, constructor, phantomId))
      .map(new CallerCodecAgentClient(definition, _))

  private def encodeConstructor(input: Constructor): Either[GolemReflectError, SchemaValue] =
    try Right(definition.constructor.toValue(input))
    catch { case NonFatal(error) => Left(GolemReflectError.SchemaEncode(error.getMessage)) }

  private def requireDurable(operation: String): Either[GolemReflectError, Unit] =
    Either.cond(
      definition.mode == AgentMode.Durable,
      (),
      GolemReflectError.Identity(s"$operation is not available for ephemeral agent types")
    )
}

final class CallerCodecAgentClient[Constructor] private[reflection] (
  definition: AgentClientDefinition[Constructor],
  transport: Transport
) {
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
