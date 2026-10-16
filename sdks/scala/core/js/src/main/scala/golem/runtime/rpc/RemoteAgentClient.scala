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

import golem.Datetime
import golem.Uuid
import golem.config.{ConfigOverride, ConfigOverrideEncoder}
import golem.FutureInterop
import golem.host.PermissionsApi.ScopeCard
import golem.host.js.schema.{JsSchemaValueTree, JsTypedAgentConfigValue, JsUuid}
import golem.runtime.rpc.host.AgentHostApi.RegisteredAgentType
import golem.runtime.rpc.host.WasmRpcApi.WasmRpcClient
import golem.runtime.rpc.host.{AgentHostApi, WasmRpcApi}

import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.JavaScriptException

final case class RemoteAgentClient(
  agentTypeName: String,
  agentId: String,
  metadata: RegisteredAgentType,
  rpc: RpcInvoker
) {
  def invokeAndAwait(
    functionName: String,
    input: JsSchemaValueTree,
    scopeCard: Option[ScopeCard] = None
  ): Either[String, Option[JsSchemaValueTree]] =
    rpc.invokeAndAwait(functionName, input, scopeCard)

  def asyncInvokeAndAwait(
    functionName: String,
    input: JsSchemaValueTree,
    scopeCard: Option[ScopeCard] = None
  ): Future[Option[JsSchemaValueTree]] =
    rpc.asyncInvokeAndAwait(functionName, input, scopeCard)

  def invokeAndAwaitWithMetadata(
    functionName: String,
    input: JsSchemaValueTree,
    scopeCard: Option[ScopeCard] = None
  ): Either[String, InvocationResult[Option[JsSchemaValueTree]]] =
    rpc.invokeAndAwaitWithMetadata(functionName, input, scopeCard)

  def cancelableAsyncInvokeAndAwait(
    functionName: String,
    input: JsSchemaValueTree,
    scopeCard: Option[ScopeCard] = None
  ): (Future[Option[JsSchemaValueTree]], CancellationToken) =
    rpc.cancelableAsyncInvokeAndAwait(functionName, input, scopeCard)

  def cancelableAsyncInvokeAndAwaitWithMetadata(
    functionName: String,
    input: JsSchemaValueTree,
    scopeCard: Option[ScopeCard] = None
  ): Either[String, CancelableAsyncInvocation[Option[JsSchemaValueTree]]] =
    rpc.asyncInvokeAndAwaitWithMetadata(functionName, input, scopeCard).map { invocation =>
      CancelableAsyncInvocation(
        invocation.metadata,
        invocation.result,
        invocation.cancellationToken
      )
    }

  def invoke(functionName: String, input: JsSchemaValueTree): Either[String, Unit] =
    rpc.invoke(functionName, input)

  def invokeWithMetadata(
    functionName: String,
    input: JsSchemaValueTree
  ): Either[String, InvocationMetadata] =
    rpc.invokeWithMetadata(functionName, input)

  def scheduleInvocation(
    datetime: Datetime,
    functionName: String,
    input: JsSchemaValueTree
  ): Either[String, Unit] =
    rpc.scheduleInvocation(datetime, functionName, input)

  def scheduleInvocationWithMetadata(
    datetime: Datetime,
    functionName: String,
    input: JsSchemaValueTree
  ): Either[String, InvocationReceipt] =
    rpc.scheduleInvocationWithMetadata(datetime, functionName, input)

  def scheduleCancelableInvocation(
    datetime: Datetime,
    functionName: String,
    input: JsSchemaValueTree
  ): Either[String, CancellationToken] =
    rpc.scheduleCancelableInvocation(datetime, functionName, input)

  def scheduleCancelableInvocationWithMetadata(
    datetime: Datetime,
    functionName: String,
    input: JsSchemaValueTree
  ): Either[String, CancelableInvocationReceipt] =
    rpc.scheduleCancelableInvocationWithMetadata(datetime, functionName, input)
}

object RemoteAgentClient {
  def resolve(agentTypeName: String, constructorPayload: JsSchemaValueTree): Either[String, RemoteAgentClient] =
    resolve(agentTypeName, constructorPayload, phantom = None)

  def resolve(
    agentTypeName: String,
    constructorPayload: JsSchemaValueTree,
    phantom: Option[Uuid]
  ): Either[String, RemoteAgentClient] =
    resolve(agentTypeName, constructorPayload, phantom, configOverrides = Nil)

  def resolve(
    agentTypeName: String,
    constructorPayload: JsSchemaValueTree,
    phantom: Option[Uuid],
    configOverrides: List[ConfigOverride]
  ): Either[String, RemoteAgentClient] =
    AgentHostApi
      .registeredAgentType(agentTypeName)
      .toRight(s"Agent type '$agentTypeName' is not registered on this host")
      .flatMap { agentType =>
        val displayTypeName = agentType.agentType.typeName
        AgentHostApi.makeAgentId(displayTypeName, constructorPayload, phantom).map { id =>
          val phantomArg: js.UndefOr[JsUuid] = phantom.fold[js.UndefOr[JsUuid]](js.undefined) { uuid =>
            JsUuid(
              js.BigInt(uuid.highBits.toString),
              js.BigInt(uuid.lowBits.toString)
            )
          }
          val jsConfig =
            if (configOverrides.isEmpty) js.Array[JsTypedAgentConfigValue]()
            else ConfigOverrideEncoder.encode(configOverrides)
          val rpcClient = WasmRpcApi.newClient(displayTypeName, constructorPayload, phantomArg, jsConfig)
          RemoteAgentClient(displayTypeName, id, agentType, new WasmRpcInvoker(rpcClient))
        }
      }

  private[rpc] final class WasmRpcInvoker(client: WasmRpcClient) extends RpcInvoker {

    /**
     * Borrows the scope card for one host call. A moved card throws
     * `IllegalStateException` before the host is called.
     */
    private def withScopeCard[A](scopeCard: Option[ScopeCard])(call: js.Any => A): A =
      scopeCard match {
        case Some(card) => card.borrowRaw(call)
        case None       => call(js.undefined)
      }

    override def invokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree,
      scopeCard: Option[ScopeCard]
    ): Either[String, Option[JsSchemaValueTree]] =
      invokeWithFallback(functionName)(fn =>
        withScopeCard(scopeCard)(client.invokeAndAwait(fn, input, _)).map(_.toOption).left.map(_.toString)
      )

    override def invokeAndAwaitWithMetadata(
      functionName: String,
      input: JsSchemaValueTree,
      scopeCard: Option[ScopeCard]
    ): Either[String, InvocationResult[Option[JsSchemaValueTree]]] =
      invokeWithFallback(functionName)(fn =>
        withScopeCard(scopeCard)(client.invokeAndAwaitWithMetadata(fn, input, _))
          .map(value => value.copy(value = value.value.toOption))
          .left
          .map(_.toString)
      )

    override def asyncInvokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree,
      scopeCard: Option[ScopeCard]
    ): Future[Option[JsSchemaValueTree]] =
      safeCall(
        withScopeCard(scopeCard)(client.asyncInvokeAndAwait(functionName, input, _)).left.map(_.toString)
      ) match {
        case Left(err)           => Future.failed(JavaScriptException(err))
        case Right(futureResult) => awaitFutureResult(futureResult)
      }

    override def asyncInvokeAndAwaitWithMetadata(
      functionName: String,
      input: JsSchemaValueTree,
      scopeCard: Option[ScopeCard]
    ): Either[String, AsyncInvocation[Option[JsSchemaValueTree]]] =
      invokeWithFallback(functionName)(fn =>
        withScopeCard(scopeCard)(client.asyncInvokeAndAwaitWithMetadata(fn, input, _)).map {
          case (metadata, futureResult) =>
            AsyncInvocation(
              metadata,
              awaitFutureResult(futureResult),
              CancellationToken.fromFunction(() => futureResult.cancel())
            )
        }.left
          .map(_.toString)
      )

    override def cancelableAsyncInvokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree,
      scopeCard: Option[ScopeCard]
    ): (Future[Option[JsSchemaValueTree]], CancellationToken) =
      safeCall(
        withScopeCard(scopeCard)(client.asyncInvokeAndAwait(functionName, input, _)).left.map(_.toString)
      ) match {
        case Left(err) =>
          (Future.failed(JavaScriptException(err)), CancellationToken.fromFunction(() => ()))
        case Right(futureResult) =>
          val token = CancellationToken.fromFunction(() => futureResult.cancel())
          (awaitFutureResult(futureResult), token)
      }

    /**
     * Drives a host `future-invoke-result` to completion. Calling `get()` is
     * guarded so a synchronous host/JS interop failure surfaces as a failed
     * `Future` rather than trapping the whole guest invocation.
     */
    private def awaitFutureResult(
      futureResult: WasmRpcApi.RawFutureInvokeResult
    ): Future[Option[JsSchemaValueTree]] =
      try {
        FutureInterop
          .fromPromise(futureResult.get())
          .map(_.toOption)(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)
          .recoverWith { case JavaScriptException(e) =>
            Future.failed(JavaScriptException(s"async RPC failed: ${WasmRpcApi.decodeRpcError(e)}"))
          }(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)
      } catch {
        case JavaScriptException(e) =>
          Future.failed(JavaScriptException(s"async RPC failed: ${WasmRpcApi.decodeRpcError(e)}"))
      }

    override def invoke(functionName: String, input: JsSchemaValueTree): Either[String, Unit] =
      invokeWithFallback(functionName)(fn => client.invoke(fn, input).left.map(_.toString))

    override def invokeWithMetadata(
      functionName: String,
      input: JsSchemaValueTree
    ): Either[String, InvocationMetadata] =
      invokeWithFallback(functionName)(fn => client.invokeWithMetadata(fn, input).left.map(_.toString))

    override def scheduleInvocation(
      datetime: Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Either[String, Unit] =
      invokeWithFallback(functionName)(fn => client.scheduleInvocation(datetime, fn, input).left.map(_.toString))

    override def scheduleInvocationWithMetadata(
      datetime: Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Either[String, InvocationReceipt] =
      invokeWithFallback(functionName)(fn =>
        client.scheduleInvocationWithMetadata(datetime, fn, input).left.map(_.toString)
      )

    override def scheduleCancelableInvocation(
      datetime: Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Either[String, CancellationToken] =
      invokeWithFallback(functionName)(fn =>
        client.scheduleCancelableInvocation(datetime, fn, input).left.map(_.toString)
      )

    override def scheduleCancelableInvocationWithMetadata(
      datetime: Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Either[String, CancelableInvocationReceipt] =
      invokeWithFallback(functionName)(fn =>
        client.scheduleCancelableInvocationWithMetadata(datetime, fn, input).left.map(_.toString)
      )
  }

  private def invokeWithFallback[A](functionName: String)(call: String => Either[String, A]): Either[String, A] =
    safeCall(call(functionName))

  private def safeCall[A](thunk: => Either[String, A]): Either[String, A] =
    try thunk
    catch {
      case JavaScriptException(err) => Left(err.toString)
    }
}
