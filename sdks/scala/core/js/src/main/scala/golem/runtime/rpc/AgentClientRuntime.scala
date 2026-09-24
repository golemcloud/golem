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

import golem.config.ConfigOverride
import golem.host.js.schema.JsSchemaValueTree
import golem.host.SchemaWireInterop
import golem.runtime.{AgentMethod, AgentType, OutputCodec, OutputMetadata}
import golem.runtime.{WireAgentClientType, WireClientMethod}
import golem.schema.wire.{ConcreteCodec, WitSchemaTypeBody, WitSchemaValueTree}
import golem.FutureInterop
import golem.Uuid
import golem.Datetime

import scala.concurrent.Future
import scala.scalajs.js
import scala.util.control.NonFatal

object AgentClientRuntime {
  private var remoteResolverOverride: Option[(String, JsSchemaValueTree) => Either[String, RemoteAgentClient]] =
    None

  def resolve[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor
  ): Either[String, ResolvedAgent[Trait]] =
    resolveWithPhantom(agentType, constructorArgs, phantom = None)

  def resolveWire[Trait, Constructor](
    agentType: WireAgentClientType[Trait, Constructor],
    constructorArgs: Constructor,
    phantom: Option[Uuid] = None,
    configOverrides: List[ConfigOverride] = Nil
  ): Either[String, WireResolvedAgent[Trait]] =
    encodeWireSync(agentType.ctorCodec, constructorArgs).flatMap { payload =>
      resolveRemote(agentType.metadata.name, payload, phantom, configOverrides)
        .map(remote => WireResolvedAgent(agentType, remote))
    }

  private def encodeWireSync[A](codec: ConcreteCodec[A], value: A): Either[String, JsSchemaValueTree] =
    if (codec.graph.typeNodes.exists(_.body.isInstanceOf[WitSchemaTypeBody.StreamType]))
      Left("live streams cannot cross fire-and-forget or scheduled agent invocation boundaries")
    else
      try Right(SchemaWireInterop.valueTreeToJs(codec.encodeValue(value)))
      catch { case NonFatal(error) => Left(String.valueOf(error.getMessage)) }

  private def encodeWireAsync[A](codec: ConcreteCodec[A], value: A): Future[JsSchemaValueTree] =
    try SchemaWireInterop.ownedValueTreeToJsAsync(codec.encodeValue(value))
    catch { case NonFatal(error) => Future.failed(error) }

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

  def resolveWithPhantom[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor,
    phantom: Option[Uuid]
  ): Either[String, ResolvedAgent[Trait]] =
    resolveWithPhantomAndConfig(agentType, constructorArgs, phantom, configOverrides = Nil)

  def resolveWithConfig[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor,
    configOverrides: List[ConfigOverride]
  ): Either[String, ResolvedAgent[Trait]] =
    resolveWithPhantomAndConfig(agentType, constructorArgs, phantom = None, configOverrides)

  def resolveWithPhantomAndConfig[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor,
    phantom: Option[Uuid],
    configOverrides: List[ConfigOverride]
  ): Either[String, ResolvedAgent[Trait]] =
    for {
      payload <- encodeInput[Constructor](agentType.constructor.inputCodec, constructorArgs)
      remote  <- resolveRemote(agentType.typeName, payload, phantom, configOverrides)
    } yield ResolvedAgent(agentType.asInstanceOf[AgentType[Trait, Any]], remote)

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
    input: In
  ): Future[JsSchemaValueTree] =
    SchemaRpcCodec.encodeArgsAsync(input)(inputCodec)

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

  private def resolveRemote(
    agentTypeName: String,
    payload: JsSchemaValueTree,
    phantom: Option[Uuid],
    configOverrides: List[ConfigOverride]
  ): Either[String, RemoteAgentClient] =
    remoteResolverOverride match {
      case Some(custom) => custom(agentTypeName, payload)
      case None         => RemoteAgentClient.resolve(agentTypeName, payload, phantom, configOverrides)
    }

  final case class ResolvedAgent[Trait](agentType: AgentType[Trait, Any], client: RemoteAgentClient) {
    def agentId: String = client.agentId

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
    def await[In, Out](method: AgentMethod[Trait, In, Out], input: In): Future[Out] =
      runAwaitable(method, input)

    def cancelableAwait[In, Out](method: AgentMethod[Trait, In, Out], input: In): (Future[Out], CancellationToken) =
      runCancelableAwaitable(method, input)

    def trigger[In](method: AgentMethod[Trait, In, _], input: In): Future[Unit] =
      runFireAndForget(method, input)

    def schedule[In](method: AgentMethod[Trait, In, _], datetime: Datetime, input: In): Future[Unit] =
      runScheduled(method, datetime, input)

    def scheduleCancelable[In](
      method: AgentMethod[Trait, In, _],
      datetime: Datetime,
      input: In
    ): Future[CancellationToken] =
      runScheduledCancelable(method, datetime, input)

    def awaitWithMetadata[In, Out](method: AgentMethod[Trait, In, Out], input: In): Future[InvocationResult[Out]] = {
      implicit val ec = scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
      encodeInputAsync(method.inputCodec, input).flatMap { params =>
        FutureInterop.fromEither(client.rpc.asyncInvokeAndAwaitWithMetadata(method.functionName, params)).flatMap { raw =>
          raw.result.flatMap(decodeOutputAsync(method.outputCodec, _)).map(InvocationResult(raw.metadata, _))
        }
      }
    }

    def cancelableAwaitWithMetadata[In, Out](
      method: AgentMethod[Trait, In, Out],
      input: In
    ): Future[CancelableAsyncInvocation[Out]] = {
      implicit val ec = scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
      encodeInputAsync(method.inputCodec, input).flatMap { params =>
        FutureInterop.fromEither(client.rpc.asyncInvokeAndAwaitWithMetadata(method.functionName, params)).map { raw =>
          CancelableAsyncInvocation(
            raw.metadata,
            raw.result.flatMap(decodeOutputAsync(method.outputCodec, _)),
            raw.cancellationToken
          )
        }
      }
    }

    def triggerWithMetadata[In](method: AgentMethod[Trait, In, _], input: In): Future[InvocationReceipt] =
      FutureInterop.fromEither(for {
        params   <- encodeInput(method.inputCodec, input)
        metadata <- client.rpc.invokeWithMetadata(method.functionName, params)
      } yield InvocationReceipt(metadata))

    def scheduleWithMetadata[In](
      method: AgentMethod[Trait, In, _],
      datetime: Datetime,
      input: In
    ): Future[InvocationReceipt] =
      FutureInterop.fromEither(for {
        params  <- encodeInput(method.inputCodec, input)
        receipt <- client.rpc.scheduleInvocationWithMetadata(datetime, method.functionName, params)
      } yield receipt)

    def scheduleCancelableWithMetadata[In](
      method: AgentMethod[Trait, In, _],
      datetime: Datetime,
      input: In
    ): Future[CancelableInvocationReceipt] =
      FutureInterop.fromEither(for {
        params  <- encodeInput(method.inputCodec, input)
        receipt <- client.rpc.scheduleCancelableInvocationWithMetadata(datetime, method.functionName, params)
      } yield receipt)

    private def runAwaitable[In, Out](method: AgentMethod[Trait, In, Out], input: In): Future[Out] = {
      implicit val ec = scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
      encodeInputAsync(method.inputCodec, input)
        .flatMap(params => client.rpc.asyncInvokeAndAwait(method.functionName, params))
        .flatMap(decodeOutputAsync(method.outputCodec, _))
    }

    private def runCancelableAwaitable[In, Out](
      method: AgentMethod[Trait, In, Out],
      input: In
    ): (Future[Out], CancellationToken) = {
      implicit val ec = scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
      var underlying  = Option.empty[CancellationToken]
      var cancelled   = false
      val token       = CancellationToken.fromFunction { () =>
        cancelled = true
        underlying.foreach(_.cancel())
      }
      val result = encodeInputAsync(method.inputCodec, input).flatMap { params =>
        val (rawFuture, rawToken) = client.rpc.cancelableAsyncInvokeAndAwait(method.functionName, params)
        underlying = Some(rawToken)
        if (cancelled) rawToken.cancel()
        rawFuture.flatMap(decodeOutputAsync(method.outputCodec, _))
      }
      (result, token)
    }

    private def runFireAndForget[In, Out0](method: AgentMethod[Trait, In, Out0], input: In): Future[Unit] = {
      val result: Either[String, Unit] = for {
        params <- encodeInput(method.inputCodec, input)
        _      <- client.rpc.invoke(method.functionName, params)
      } yield ()
      FutureInterop.fromEither(result)
    }

    private def runScheduled[In, Out0](
      method: AgentMethod[Trait, In, Out0],
      datetime: Datetime,
      input: In
    ): Future[Unit] = {
      val result: Either[String, Unit] = for {
        params <- encodeInput(method.inputCodec, input)
        _      <- client.rpc.scheduleInvocation(datetime, method.functionName, params)
      } yield ()
      FutureInterop.fromEither(result)
    }

    private def runScheduledCancelable[In, Out0](
      method: AgentMethod[Trait, In, Out0],
      datetime: Datetime,
      input: In
    ): Future[CancellationToken] = {
      val result: Either[String, CancellationToken] = for {
        params <- encodeInput(method.inputCodec, input)
        token  <- client.rpc.scheduleCancelableInvocation(datetime, method.functionName, params)
      } yield token
      FutureInterop.fromEither(result)
    }
  }

  final case class WireResolvedAgent[Trait](agentType: WireAgentClientType[Trait, ?], client: RemoteAgentClient) {
    def agentId: String = client.agentId

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
      implicit val ec = scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
      encodeWireAsync(method.input, input)
        .flatMap(client.rpc.asyncInvokeAndAwait(method.name, _))
        .map(r => decodeWire(method.output, r))
    }

    def cancelableAwait[In, Out](
      method: WireClientMethod[Trait] { type Input = In; type Output = Out },
      input: In
    ): (Future[Out], CancellationToken) = {
      implicit val ec = scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
      var underlying  = Option.empty[CancellationToken]
      var cancelled   = false
      val token       = CancellationToken.fromFunction { () => cancelled = true; underlying.foreach(_.cancel()) }
      val result      = encodeWireAsync(method.input, input).flatMap { params =>
        val (future, raw) = client.rpc.cancelableAsyncInvokeAndAwait(method.name, params)
        underlying = Some(raw)
        if (cancelled) raw.cancel()
        future.map(r => decodeWire(method.output, r))
      }
      (result, token)
    }

    private def immediate[In, A](method: WireClientMethod[Trait] { type Input = In }, input: In)(
      invoke: JsSchemaValueTree => Either[String, A]
    ): Future[A] = FutureInterop.fromEither(encodeWireSync(method.input, input).flatMap(invoke))

    def trigger[In](method: WireClientMethod[Trait] { type Input = In }, input: In): Future[Unit] =
      immediate(method, input)(client.rpc.invoke(method.name, _))
    def schedule[In](method: WireClientMethod[Trait] { type Input = In }, when: Datetime, input: In): Future[Unit] =
      immediate(method, input)(client.rpc.scheduleInvocation(when, method.name, _))
    def scheduleCancelable[In](
      method: WireClientMethod[Trait] { type Input = In },
      when: Datetime,
      input: In
    ): Future[CancellationToken] =
      immediate(method, input)(client.rpc.scheduleCancelableInvocation(when, method.name, _))

    def awaitWithMetadata[In, Out](
      method: WireClientMethod[Trait] { type Input = In; type Output = Out },
      input: In
    ): Future[InvocationResult[Out]] = {
      implicit val ec = scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
      encodeWireAsync(method.input, input)
        .flatMap(params => FutureInterop.fromEither(client.rpc.asyncInvokeAndAwaitWithMetadata(method.name, params)))
        .flatMap(raw => raw.result.map(r => decodeWire(method.output, r)).map(InvocationResult(raw.metadata, _)))
    }
    def cancelableAwaitWithMetadata[In, Out](
      method: WireClientMethod[Trait] { type Input = In; type Output = Out },
      input: In
    ): Future[CancelableAsyncInvocation[Out]] = {
      implicit val ec = scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
      encodeWireAsync(method.input, input)
        .flatMap(params => FutureInterop.fromEither(client.rpc.asyncInvokeAndAwaitWithMetadata(method.name, params)))
        .map { raw =>
          CancelableAsyncInvocation(
            raw.metadata,
            raw.result.map(r => decodeWire(method.output, r)),
            raw.cancellationToken
          )
        }
    }
    def triggerWithMetadata[In](
      method: WireClientMethod[Trait] { type Input = In },
      input: In
    ): Future[InvocationReceipt] =
      immediate(method, input)(p => client.rpc.invokeWithMetadata(method.name, p).map(InvocationReceipt(_)))
    def scheduleWithMetadata[In](
      method: WireClientMethod[Trait] { type Input = In },
      when: Datetime,
      input: In
    ): Future[InvocationReceipt] =
      immediate(method, input)(client.rpc.scheduleInvocationWithMetadata(when, method.name, _))
    def scheduleCancelableWithMetadata[In](
      method: WireClientMethod[Trait] { type Input = In },
      when: Datetime,
      input: In
    ): Future[CancelableInvocationReceipt] =
      immediate(method, input)(client.rpc.scheduleCancelableInvocationWithMetadata(when, method.name, _))
  }

  private[rpc] object TestHooks {
    def encodeImmediate[A](codec: ConcreteCodec[A], value: A): Either[String, JsSchemaValueTree] =
      encodeWireSync(codec, value)

    def withRemoteResolver[T](
      resolver: (String, JsSchemaValueTree) => Either[String, RemoteAgentClient]
    )(thunk: => T): T = {
      val previous = remoteResolverOverride
      remoteResolverOverride = Some(resolver)
      try thunk
      finally remoteResolverOverride = previous
    }
  }
}
