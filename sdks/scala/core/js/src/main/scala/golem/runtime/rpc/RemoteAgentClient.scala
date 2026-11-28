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

import golem.{AgentId, Datetime}
import golem.concurrent.GolemExecutionContext
import golem.host.js.JsAgentId
import golem.host.PermissionsApi.ScopeCard
import golem.host.js.schema.JsSchemaValueTree
import golem.runtime.rpc.host.WasmRpcApi
import golem.runtime.rpc.host.WasmRpcApi.WasmRpcClient

import scala.concurrent.Future
import scala.scalajs.js
import scala.util.Try
import scala.util.control.NonFatal

/**
 * A client of one remote agent on raw value trees. Every operation fails with
 * an [[AgentRpcException]] when the host reports an `rpc-error`.
 */
final case class RemoteAgentClient(
  agentTypeName: String,
  rpc: RpcInvoker
) {

  /** The ID of the target agent; see [[RpcInvoker.agentId]]. */
  def agentId(): Future[AgentId] =
    rpc.agentId()

  /**
   * Invokes the method and awaits its result together with its final identity.
   * The call borrows `scopeCard`.
   */
  def invokeAndAwait(
    functionName: String,
    input: JsSchemaValueTree,
    scopeCard: Option[ScopeCard] = None
  ): Future[InvocationResult[Option[JsSchemaValueTree]]] =
    rpc.invokeAndAwait(functionName, input, scopeCard)

  /**
   * Triggers the method, and returns its handle once the target has durably
   * accepted it. `decode` decodes the awaited result of the handle. Never lends
   * a scope card.
   */
  def trigger[Out](functionName: String, input: JsSchemaValueTree)(
    decode: Option[JsSchemaValueTree] => Out
  ): Future[Invocation[Out]] =
    rpc.trigger(functionName, input).map(_.mapResult(decode))(GolemExecutionContext.global)

  /**
   * Schedules the method for `datetime`, and returns its handle once the target
   * has durably accepted it. `decode` decodes the awaited result of the handle.
   * Never lends a scope card.
   */
  def schedule[Out](datetime: Datetime, functionName: String, input: JsSchemaValueTree)(
    decode: Option[JsSchemaValueTree] => Out
  ): Future[Invocation[Out]] =
    rpc.schedule(datetime, functionName, input).map(_.mapResult(decode))(GolemExecutionContext.global)
}

object RemoteAgentClient {

  /**
   * `knownAgentId` is the ID the host reported when it made the client; without
   * it, [[agentId]] asks the host through the client.
   */
  private[rpc] final class WasmRpcInvoker(client: WasmRpcClient, knownAgentId: Option[AgentId]) extends RpcInvoker {
    private var cachedAgentId: Option[AgentId] = knownAgentId

    private implicit val ec: scala.concurrent.ExecutionContext = GolemExecutionContext.global

    override def agentId(): Future[AgentId] =
      cachedAgentId match {
        case Some(id) => Future.successful(id)
        case None     =>
          attempt(client.agentId()).flatMap(succeeded).map { raw =>
            val id = JsAgentId.toAgentId(raw)
            cachedAgentId = Some(id)
            id
          }
      }

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
    ): Future[InvocationResult[Option[JsSchemaValueTree]]] =
      attempt(withScopeCard(scopeCard)(client.invokeAndAwait(functionName, input, _)))
        .flatMap(succeeded)
        .map(result => result.copy(value = result.value.toOption))

    override def trigger(
      functionName: String,
      input: JsSchemaValueTree
    ): Future[Invocation[Option[JsSchemaValueTree]]] =
      attempt(client.trigger(functionName, input)).flatMap(succeeded).flatMap(handle)

    override def schedule(
      datetime: Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Future[Invocation[Option[JsSchemaValueTree]]] =
      attempt(client.schedule(datetime, functionName, input)).flatMap(succeeded).flatMap(handle)

    private def handle(raw: WasmRpcApi.RawInvocation): Future[Invocation[Option[JsSchemaValueTree]]] =
      Future.fromTry(Try(Invocation.fromHost(raw)))
  }

  private def succeeded[A](result: Either[WasmRpcApi.RpcError, A]): Future[A] =
    result.fold(error => Future.failed(AgentRpcException(AgentRpcError.fromRpcError(error))), Future.successful)

  private def attempt[A](future: => Future[A]): Future[A] =
    try future
    catch { case NonFatal(error) => Future.failed(error) }
}
