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
import golem.runtime.{AgentMethod, AgentType, OutputCodec, OutputMetadata}
import golem.FutureInterop
import golem.Uuid
import golem.Datetime

import scala.concurrent.Future
import scala.scalajs.js
import scala.util.control.NonFatal

object AgentClientRuntime {
  @volatile private var remoteResolverOverride
    : Option[(String, JsSchemaValueTree) => Either[String, RemoteAgentClient]] =
    None

  def resolve[Trait, Constructor](
    agentType: AgentType[Trait, Constructor],
    constructorArgs: Constructor
  ): Either[String, ResolvedAgent[Trait]] =
    resolveWithPhantom(agentType, constructorArgs, phantom = None)

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

    private def runAwaitable[In, Out](method: AgentMethod[Trait, In, Out], input: In): Future[Out] = {
      // The default await path uses the synchronous host `invoke-and-await`
      // import (fully wrapped in `Either`), matching the documented "always
      // invoke via invoke-and-await" intent. The async `future-invoke-result`
      // path is reserved for `cancelableAwait`, where cancellation is needed.
      val result: Either[String, Out] = for {
        params <- encodeInput(method.inputCodec, input)
        raw    <- client.rpc.invokeAndAwait(method.functionName, params)
        value  <- decodeOutput(method.outputCodec, raw)
      } yield value
      FutureInterop.fromEither(result)
    }

    private def runCancelableAwaitable[In, Out](
      method: AgentMethod[Trait, In, Out],
      input: In
    ): (Future[Out], CancellationToken) =
      encodeInput(method.inputCodec, input) match {
        case Left(err) =>
          (FutureInterop.failed(err), CancellationToken.fromFunction(() => ()))
        case Right(params) =>
          val (rawFuture, token) = client.rpc.cancelableAsyncInvokeAndAwait(method.functionName, params)
          val mappedFuture       = rawFuture.map { raw =>
            decodeOutput(method.outputCodec, raw) match {
              case Left(err)    => throw scala.scalajs.js.JavaScriptException(err)
              case Right(value) => value
            }
          }(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)
          (mappedFuture, token)
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

  private[rpc] object TestHooks {
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
