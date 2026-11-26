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

package golem.runtime.rpc

import golem.host.PermissionsApi.ScopeCard
import golem.host.js.schema.JsSchemaValueTree
import golem.host.SchemaWireInterop
import golem.runtime.{AgentMethod, AgentType, OutputCodec, OutputMetadata}
import golem.runtime.{WireAgentClientType, WireClientMethod}
import golem.schema.wire.{ConcreteCodec, WitSchemaValueTree}
import golem.{AgentId, Datetime}

import scala.concurrent.Future
import scala.scalajs.js
import scala.util.control.NonFatal

object AgentClientRuntime {
  private var remoteResolverOverride: Option[(String, JsSchemaValueTree) => Either[AgentRpcError, RemoteAgentClient]] =
    None

  /**
   * A client of the keyed agent of the constructor parameters. A durable target
   * is created when it is missing. Every invocation of an ephemeral target runs
   * on a new one-shot instance.
   */
  def upsertWire[Trait, Constructor](
    agentType: WireAgentClientType[Trait, Constructor],
    constructorArgs: Constructor
  ): Either[AgentRpcError, WireResolvedAgent[Trait]] =
    encodeWireSync(agentType.ctorCodec, agentType.ctorContainsStream, constructorArgs).left
      .map(AgentRpcError.InvalidInput(_))
      .flatMap(upsertRemote(agentType.metadata.name, _))
      .map(remote => WireResolvedAgent(agentType, remote))

  /**
   * Creates a durable agent and returns a client of it. Fails with
   * [[AgentRpcException]].
   */
  def createWire[Trait, Constructor](
    agentType: WireAgentClientType[Trait, Constructor],
    constructorArgs: Constructor,
    options: CreateOptions
  ): Future[WireResolvedAgent[Trait]] =
    encodeWireSync(agentType.ctorCodec, agentType.ctorContainsStream, constructorArgs) match {
      case Left(error)    => Future.failed(AgentRpcException(AgentRpcError.InvalidInput(error)))
      case Right(payload) =>
        AgentIdentity
          .create(agentType.metadata.name, payload, options)
          .map(remote => WireResolvedAgent(agentType, remote))(
            scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
          )
    }

  /**
   * A client of the existing agent with the given ID. Never creates the agent.
   * An unknown ID, or the ID of an agent of another type, fails with
   * [[AgentRpcError.NotFound]].
   */
  def getByIdWire[Trait](
    agentType: WireAgentClientType[Trait, ?],
    agentId: AgentId
  ): Either[AgentRpcError, WireResolvedAgent[Trait]] =
    AgentIdentity
      .getById(agentType.metadata.name, agentId)
      .map(remote => WireResolvedAgent(agentType, remote))

  /**
   * Bind the current durable incarnation without creating it or waiting for
   * initialization.
   *
   * Use this to schedule work on self from inside the constructor. Normal
   * client construction reuses an initialized agent without rerunning its
   * constructor, but waits for an unfinished constructor. Calling it on self
   * from that constructor would therefore deadlock.
   *
   * Scheduled work runs after construction finishes. This does not make awaited
   * self-calls reentrant or bypass method permissions. A different client type
   * returns an error. Replay preserves the recorded incarnation.
   */
  def bindSelfWire[Trait, Constructor](
    agentType: WireAgentClientType[Trait, Constructor]
  ): Either[String, WireResolvedAgent[Trait]] =
    AgentIdentity.bindSelf(agentType.metadata.name).map(remote => WireResolvedAgent(agentType, remote))

  /**
   * Bind self using a typed reflective agent definition, without waiting for
   * initialization.
   *
   * Use this for constructor-time self-scheduling, as described by
   * [[bindSelfWire]]. Normal client construction on self would wait for the
   * executing constructor to finish and deadlock. This does not make awaited
   * self-calls reentrant.
   */
  def bindSelf[Trait, Constructor](
    agentType: AgentType[Trait, Constructor]
  ): Either[String, ResolvedAgent[Trait]] =
    AgentIdentity.bindSelf(agentType.typeName).map { remote =>
      ResolvedAgent(agentType.asInstanceOf[AgentType[Trait, Any]], remote)
    }

  private def encodeWireSync[A](
    codec: ConcreteCodec[A],
    containsStream: Boolean,
    value: A
  ): Either[String, JsSchemaValueTree] =
    if (containsStream)
      Left("live streams cannot cross fire-and-forget or scheduled agent invocation boundaries")
    else
      try Right(SchemaWireInterop.valueTreeToJs(codec.encodeValue(value)))
      catch { case NonFatal(error) => Left(String.valueOf(error.getMessage)) }

  private val ScopeCardMoved =
    "the scope card has been released or moved; derive a new scope card in this invocation"

  /**
   * The lowered input of an awaited call. Lowering takes the owned handles of
   * argument cards, tokens and secrets; [[call]] gives them back when the scope
   * card is gone, so a failure before the host call never loses an argument.
   */
  private[golem] final class AwaitedInput(
    wit: WitSchemaValueTree,
    lowered: JsSchemaValueTree,
    scopeCard: Option[ScopeCard]
  ) {

    /** Checks the scope card again in the same turn as the host call. */
    def call[A](host: JsSchemaValueTree => A): A = {
      if (scopeCard.exists(!_.isPresent)) {
        SchemaWireInterop.returnCapabilityHandles(wit, lowered)
        throw new IllegalStateException(ScopeCardMoved)
      }
      host(lowered)
    }
  }

  /**
   * Checks the scope card before encoding, so a moved card never consumes an
   * argument.
   */
  private[golem] def awaitedInput(scopeCard: Option[ScopeCard])(
    encode: => Future[(WitSchemaValueTree, JsSchemaValueTree)]
  ): Future[AwaitedInput] =
    if (scopeCard.exists(!_.isPresent)) Future.failed(new IllegalStateException(ScopeCardMoved))
    else {
      implicit val ec = golem.concurrent.GolemExecutionContext.global
      (try encode
      catch { case NonFatal(error) => Future.failed(error) }).map { case (wit, lowered) =>
        new AwaitedInput(wit, lowered, scopeCard)
      }
    }

  /**
   * The input of an awaited call from a wire tree, used by every awaited client
   * path: the scope card is checked before encoding and again with
   * [[AwaitedInput.call]].
   */
  private[golem] def awaitedWireInput(scopeCard: Option[ScopeCard])(
    wit: => WitSchemaValueTree
  ): Future[AwaitedInput] =
    awaitedInput(scopeCard) {
      implicit val ec = golem.concurrent.GolemExecutionContext.global
      val tree        = wit
      SchemaWireInterop.ownedValueTreeToJsAsync(tree).map(lowered => (tree, lowered))
    }

  /** Like [[awaitedWireInput]], from a schema value. */
  private[golem] def awaitedValueInput(scopeCard: Option[ScopeCard])(
    value: => golem.schema.SchemaValue
  ): Future[AwaitedInput] =
    awaitedInput(scopeCard)(golem.runtime.autowire.SchemaPayload.encodeValueWithWireAsync(value))

  private def encodeWireAsync[A](
    codec: ConcreteCodec[A],
    value: A,
    scopeCard: Option[ScopeCard]
  ): Future[AwaitedInput] =
    awaitedWireInput(scopeCard)(codec.encodeValue(value))

  private def decodeWire[A](codec: Option[ConcreteCodec[A]], value: Option[JsSchemaValueTree]): A =
    codec match {
      case None =>
        if (value.nonEmpty) throw new IllegalArgumentException("agent result unexpectedly contained a value")
        ().asInstanceOf[A]
      case Some(concrete) =>
        concrete.decode(
          value
            .map(SchemaWireInterop.valueTreeFromJs)
            .getOrElse(
              throw new IllegalArgumentException("agent result did not contain a value")
            )
        )
    }

  /** Like [[upsertWire]], for a reflective agent type. */
  def upsert[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor
  ): Either[AgentRpcError, ResolvedAgent[Trait]] =
    encodeInput[Constructor](agentType.constructor.inputCodec, constructorArgs).left
      .map(AgentRpcError.InvalidInput(_))
      .flatMap(upsertRemote(agentType.typeName, _))
      .map(remote => ResolvedAgent(agentType.asInstanceOf[AgentType[Trait, Any]], remote))

  /** Like [[createWire]], for a reflective agent type. */
  def create[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor,
    options: CreateOptions
  ): Future[ResolvedAgent[Trait]] =
    encodeInput[Constructor](agentType.constructor.inputCodec, constructorArgs) match {
      case Left(error)    => Future.failed(AgentRpcException(AgentRpcError.InvalidInput(error)))
      case Right(payload) =>
        AgentIdentity
          .create(agentType.typeName, payload, options)
          .map(remote => ResolvedAgent(agentType.asInstanceOf[AgentType[Trait, Any]], remote))(
            scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
          )
    }

  /** Like [[getByIdWire]], for a reflective agent type. */
  def getById[Trait](agentType: AgentType[Trait, ?], agentId: AgentId): Either[AgentRpcError, ResolvedAgent[Trait]] =
    AgentIdentity
      .getById(agentType.typeName, agentId)
      .map(remote => ResolvedAgent(agentType.asInstanceOf[AgentType[Trait, Any]], remote))

  private def encodeInput[In](
    inputCodec: golem.runtime.InputRecordCodec[In],
    input: In
  ): Either[String, JsSchemaValueTree] =
    if (inputCodec.graph.containsStream)
      Left("live streams cannot cross fire-and-forget or scheduled agent invocation boundaries")
    else
      // `SchemaRpcCodec.encodeArgs` throws on a malformed positional record; keep
      // local encode errors as `Left` rather than throwing synchronously.
      try Right(SchemaRpcCodec.encodeArgs(input)(inputCodec))
      catch {
        case js.JavaScriptException(err) => Left(err.toString)
        case NonFatal(err)               => Left(err.getMessage)
      }

  private def outputContainsStream(outputCodec: OutputCodec[?]): Boolean =
    outputCodec.metadata match {
      case OutputMetadata.Unit          => false
      case OutputMetadata.Single(graph) => graph.containsStream
    }

  private def decodeOutput[Out](
    outputCodec: OutputCodec[Out],
    result: Option[JsSchemaValueTree]
  ): Either[String, Out] =
    outputCodec.metadata match {
      case OutputMetadata.Unit      => SchemaRpcCodec.decodeUnitResult(result).map(_.asInstanceOf[Out])
      case OutputMetadata.Single(_) => SchemaRpcCodec.decodeSingleResult[Out](result)(outputCodec.from.get)
    }

  private def encodeInputAsync[In](
    inputCodec: golem.runtime.InputRecordCodec[In],
    input: In,
    scopeCard: Option[ScopeCard]
  ): Future[AwaitedInput] =
    awaitedValueInput(scopeCard)(inputCodec.toValue(input))

  private def decodeOutputAsync[Out](outputCodec: OutputCodec[Out], result: Option[JsSchemaValueTree]): Future[Out] =
    outputCodec.metadata match {
      case OutputMetadata.Unit      => Future.successful(().asInstanceOf[Out])
      case OutputMetadata.Single(_) =>
        Future.fromTry(
          scala.util.Try(
            SchemaRpcCodec
              .decodeSingleResult[Out](result)(outputCodec.from.get)
              .fold(err => throw js.JavaScriptException(err), identity)
          )
        )
    }

  private def upsertRemote(
    agentTypeName: String,
    payload: JsSchemaValueTree
  ): Either[AgentRpcError, RemoteAgentClient] =
    remoteResolverOverride match {
      case Some(custom) => custom(agentTypeName, payload)
      case None         => AgentIdentity.upsert(agentTypeName, payload)
    }

  /**
   * `scopeCard` is attached to the awaited invocations (`await` and
   * `awaitWithMetadata`) and borrowed for each call. Triggered and scheduled
   * invocations never carry a scope card.
   */
  final case class ResolvedAgent[Trait](
    agentType: AgentType[Trait, Any],
    client: RemoteAgentClient,
    scopeCard: Option[ScopeCard] = None
  ) {

    /** The ID of the target agent; see [[RemoteAgentClient.agentId]]. */
    def agentId(): Future[AgentId] = client.agentId()

    /**
     * A view of this agent whose awaited invocations borrow `card` as the scope
     * card.
     */
    def withScopeCard(card: ScopeCard): ResolvedAgent[Trait] = copy(scopeCard = Some(card))

    /** A view of this agent whose awaited invocations carry no scope card. */
    def withoutScopeCard: ResolvedAgent[Trait] = copy(scopeCard = None)

    private lazy val methodsByName: Map[String, AgentType.AnyMethod[Trait]] =
      agentType.methods.iterator.map(m => m.metadata.name -> m).toMap

    private[rpc] def methodByName[In, Out](name: String): AgentMethod[Trait, In, Out] =
      methodsByName
        .getOrElse(name, throw new IllegalStateException(s"Method definition for $name not found"))
        .asInstanceOf[AgentMethod[Trait, In, Out]]

    /**
     * Always invoke via "invoke-and-await" regardless of `method.invocation`.
     *
     * This enables "await/trigger/schedule for any method" APIs (TS/Rust
     * parity).
     */
    def await[In, Out](method: AgentMethod[Trait, In, Out], input: In): Future[Out] = {
      implicit val ec = golem.concurrent.GolemExecutionContext.global
      awaitWithMetadata(method, input).map(_.value)
    }

    def awaitWithMetadata[In, Out](method: AgentMethod[Trait, In, Out], input: In): Future[InvocationResult[Out]] = {
      implicit val ec = golem.concurrent.GolemExecutionContext.global
      encodeInputAsync(method.inputCodec, input, scopeCard)
        .flatMap(params => params.call(client.rpc.invokeAndAwait(method.functionName, _, scopeCard)))
        .flatMap(raw => decodeOutputAsync(method.outputCodec, raw.value).map(InvocationResult(raw.metadata, _)))
    }

    /**
     * Triggers the method, and returns its [[Invocation]] handle once the
     * target has durably accepted it. Never lends the scope card.
     */
    def trigger[In, Out](method: AgentMethod[Trait, In, Out], input: In): Future[Invocation[Out]] =
      start(method, input)(client.rpc.trigger(method.functionName, _))

    /**
     * Schedules the method for `datetime`, and returns its [[Invocation]]
     * handle once the target has durably accepted it. Never lends the scope
     * card.
     */
    def schedule[In, Out](method: AgentMethod[Trait, In, Out], datetime: Datetime, input: In): Future[Invocation[Out]] =
      start(method, input)(client.rpc.schedule(datetime, method.functionName, _))

    private def start[In, Out](method: AgentMethod[Trait, In, Out], input: In)(
      call: JsSchemaValueTree => Future[Invocation[Option[JsSchemaValueTree]]]
    ): Future[Invocation[Out]] = {
      implicit val ec = golem.concurrent.GolemExecutionContext.global
      val params      =
        if (outputContainsStream(method.outputCodec))
          Left("live streams cannot cross fire-and-forget or scheduled agent invocation boundaries")
        else encodeInput(method.inputCodec, input)
      params match {
        case Left(error)   => Future.failed(new IllegalArgumentException(error))
        case Right(params) =>
          attempt(call(params)).map(_.mapResult { result =>
            decodeOutput(method.outputCodec, result).fold(error => throw new IllegalArgumentException(error), identity)
          })
      }
    }
  }

  /**
   * `scopeCard` is attached to the awaited invocations (`await` and
   * `awaitWithMetadata`) and borrowed for each call. Triggered and scheduled
   * invocations never carry a scope card.
   */
  final case class WireResolvedAgent[Trait](
    agentType: WireAgentClientType[Trait, ?],
    client: RemoteAgentClient,
    scopeCard: Option[ScopeCard] = None
  ) {

    /** The ID of the target agent; see [[RemoteAgentClient.agentId]]. */
    def agentId(): Future[AgentId] = client.agentId()

    /**
     * A view of this agent whose awaited invocations borrow `card` as the scope
     * card.
     */
    def withScopeCard(card: ScopeCard): WireResolvedAgent[Trait] = copy(scopeCard = Some(card))

    /** A view of this agent whose awaited invocations carry no scope card. */
    def withoutScopeCard: WireResolvedAgent[Trait] = copy(scopeCard = None)

    private lazy val methods = agentType.methods.iterator.map(m => m.name -> m).toMap

    private[rpc] def methodByName[In, Out](name: String): WireClientMethod[Trait] {
      type Input = In; type Output = Out
    } =
      methods
        .getOrElse(name, throw new IllegalStateException(s"Method definition for $name not found"))
        .asInstanceOf[WireClientMethod[Trait] { type Input = In; type Output = Out }]

    def await[In, Out](
      method: WireClientMethod[Trait] { type Input = In; type Output = Out },
      input: In
    ): Future[Out] = {
      implicit val ec = golem.concurrent.GolemExecutionContext.global
      awaitWithMetadata(method, input).map(_.value)
    }

    def awaitWithMetadata[In, Out](
      method: WireClientMethod[Trait] { type Input = In; type Output = Out },
      input: In
    ): Future[InvocationResult[Out]] = {
      implicit val ec = golem.concurrent.GolemExecutionContext.global
      encodeWireAsync(method.input, input, scopeCard)
        .flatMap(params => params.call(client.rpc.invokeAndAwait(method.name, _, scopeCard)))
        .map(raw => InvocationResult(raw.metadata, decodeWire(method.output, raw.value)))
    }

    private def start[In, Out](method: WireClientMethod[Trait] { type Input = In; type Output = Out }, input: In)(
      call: JsSchemaValueTree => Future[Invocation[Option[JsSchemaValueTree]]]
    ): Future[Invocation[Out]] = {
      implicit val ec = golem.concurrent.GolemExecutionContext.global
      encodeWireSync(method.input, method.inputContainsStream || method.outputContainsStream, input) match {
        case Left(error)   => Future.failed(new IllegalArgumentException(error))
        case Right(params) => attempt(call(params)).map(_.mapResult(decodeWire(method.output, _)))
      }
    }

    /**
     * Triggers the method, and returns its [[Invocation]] handle once the
     * target has durably accepted it. Never lends the scope card.
     */
    def trigger[In, Out](
      method: WireClientMethod[Trait] { type Input = In; type Output = Out },
      input: In
    ): Future[Invocation[Out]] =
      start(method, input)(client.rpc.trigger(method.name, _))

    /**
     * Schedules the method for `when`, and returns its [[Invocation]] handle
     * once the target has durably accepted it. Never lends the scope card.
     */
    def schedule[In, Out](
      method: WireClientMethod[Trait] { type Input = In; type Output = Out },
      when: Datetime,
      input: In
    ): Future[Invocation[Out]] =
      start(method, input)(client.rpc.schedule(when, method.name, _))
  }

  private def attempt[A](future: => Future[A]): Future[A] =
    try future
    catch { case NonFatal(error) => Future.failed(error) }

  private[rpc] object TestHooks {
    def encodeImmediate[A](
      codec: ConcreteCodec[A],
      containsStream: Boolean,
      value: A
    ): Either[String, JsSchemaValueTree] =
      encodeWireSync(codec, containsStream, value)

    def withRemoteResolver[T](
      resolver: (String, JsSchemaValueTree) => Either[AgentRpcError, RemoteAgentClient]
    )(thunk: => T): T = {
      val previous = remoteResolverOverride
      remoteResolverOverride = Some(resolver)
      try thunk
      finally remoteResolverOverride = previous
    }
  }
}
