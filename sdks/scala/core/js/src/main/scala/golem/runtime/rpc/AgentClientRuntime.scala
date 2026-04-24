/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.runtime.rpc

import golem.config.ConfigOverride
import golem.data.GolemSchema
import golem.host.js._
import golem.runtime.{AgentMethod, AgentType}
import golem.FutureInterop
import golem.Uuid
import golem.Datetime

import scala.concurrent.Future
import scala.scalajs.js

object AgentClientRuntime {
  @volatile private var remoteResolverOverride: Option[(String, JsDataValue) => Either[String, RemoteAgentClient]] =
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
  ): Either[String, ResolvedAgent[Trait]] = {
    implicit val ctorSchema: GolemSchema[Constructor] = agentType.constructor.schema

    for {
      payload <- {
        val any = constructorArgs.asInstanceOf[Any]
        if (any == ((): Unit)) {
          Right(JsDataValue.tuple(new js.Array[JsElementValue]()))
        } else {
          RpcValueCodec.encodeArgs[Constructor](constructorArgs)
        }
      }
      remote <- resolveRemote(agentType.typeName, payload, phantom, configOverrides)
    } yield ResolvedAgent(agentType.asInstanceOf[AgentType[Trait, Any]], remote)
  }

  private def resolveRemote(
    agentTypeName: String,
    payload: JsDataValue,
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

    def scheduleCancelable[In](method: AgentMethod[Trait, In, _], datetime: Datetime, input: In): Future[CancellationToken] =
      runScheduledCancelable(method, datetime, input)

    private def runAwaitable[In, Out](method: AgentMethod[Trait, In, Out], input: In): Future[Out] = {
      implicit val inSchema: GolemSchema[In] = method.inputSchema

      val functionName = method.functionName
      RpcValueCodec.encodeArgs(input) match {
        case Left(err) => FutureInterop.failed(err)
        case Right(params) =>
          client.rpc.asyncInvokeAndAwait(functionName, params).map { raw =>
            implicit val outSchema: GolemSchema[Out] = method.outputSchema
            RpcValueCodec.decodeResult[Out](raw) match {
              case Left(err)    => throw scala.scalajs.js.JavaScriptException(err)
              case Right(value) => value
            }
          }(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)
      }
    }

    private def runCancelableAwaitable[In, Out](method: AgentMethod[Trait, In, Out], input: In): (Future[Out], CancellationToken) = {
      implicit val inSchema: GolemSchema[In] = method.inputSchema

      val functionName = method.functionName
      RpcValueCodec.encodeArgs(input) match {
        case Left(err) =>
          (FutureInterop.failed(err), CancellationToken.fromFunction(() => ()))
        case Right(params) =>
          val (rawFuture, token) = client.rpc.cancelableAsyncInvokeAndAwait(functionName, params)
          val mappedFuture = rawFuture.map { raw =>
            implicit val outSchema: GolemSchema[Out] = method.outputSchema
            RpcValueCodec.decodeResult[Out](raw) match {
              case Left(err)    => throw scala.scalajs.js.JavaScriptException(err)
              case Right(value) => value
            }
          }(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)
          (mappedFuture, token)
      }
    }

    private def runFireAndForget[In, Out0](method: AgentMethod[Trait, In, Out0], input: In): Future[Unit] = {
      implicit val inSchema: GolemSchema[In] = method.inputSchema

      val functionName                 = method.functionName
      val result: Either[String, Unit] = for {
        params <- RpcValueCodec.encodeArgs(input)
        _      <- client.rpc.invoke(functionName, params)
      } yield ()

      FutureInterop.fromEither(result)
    }

    private def runScheduled[In, Out0](
      method: AgentMethod[Trait, In, Out0],
      datetime: Datetime,
      input: In
    ): Future[Unit] = {
      implicit val inSchema: GolemSchema[In] = method.inputSchema

      val functionName                 = method.functionName
      val result: Either[String, Unit] = for {
        params <- RpcValueCodec.encodeArgs(input)
        _      <- client.rpc.scheduleInvocation(datetime, functionName, params)
      } yield ()

      FutureInterop.fromEither(result)
    }

    private def runScheduledCancelable[In, Out0](
      method: AgentMethod[Trait, In, Out0],
      datetime: Datetime,
      input: In
    ): Future[CancellationToken] = {
      implicit val inSchema: GolemSchema[In] = method.inputSchema

      val functionName                               = method.functionName
      val result: Either[String, CancellationToken] = for {
        params <- RpcValueCodec.encodeArgs(input)
        token  <- client.rpc.scheduleCancelableInvocation(datetime, functionName, params)
      } yield token

      FutureInterop.fromEither(result)
    }
  }

  private[rpc] object TestHooks {
    def withRemoteResolver[T](resolver: (String, JsDataValue) => Either[String, RemoteAgentClient])(thunk: => T): T = {
      val previous = remoteResolverOverride
      remoteResolverOverride = Some(resolver)
      try thunk
      finally remoteResolverOverride = previous
    }

  }
}
