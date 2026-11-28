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
import golem.host.PermissionsApi.ScopeCard
import golem.host.js.schema.JsSchemaValueTree

import scala.concurrent.Future

/**
 * The `golem:agent/host@2.0.0` RPC client surface. Method inputs are a single
 * `schema-value-tree` (the parameter-list record) and awaited results are an
 * `option<schema-value-tree>` (modelled as a Scala [[Option]]): `None` for a
 * `unit` output, `Some(tree)` for a `single` output.
 *
 * Only the awaited call takes a `scopeCard`. The implementation borrows the
 * card for the host call and leaves it usable. Every operation fails with an
 * [[AgentRpcException]] when the host reports an `rpc-error`.
 */
private[rpc] trait RpcInvoker {

  /** The ID of the target agent. */
  def agentId(): Future[AgentId] =
    Future.failed(AgentRpcException(AgentRpcError.Unknown("unsupported", Some("agent ID is not supported"))))

  /**
   * Invokes the method and awaits its result together with its final identity.
   */
  def invokeAndAwait(
    functionName: String,
    input: JsSchemaValueTree,
    scopeCard: Option[ScopeCard]
  ): Future[InvocationResult[Option[JsSchemaValueTree]]]

  /**
   * Triggers the method, and returns its handle once the target has durably
   * accepted it.
   */
  def trigger(functionName: String, input: JsSchemaValueTree): Future[Invocation[Option[JsSchemaValueTree]]]

  /**
   * Schedules the method for `datetime`, and returns its handle once the target
   * has durably accepted it.
   */
  def schedule(
    datetime: Datetime,
    functionName: String,
    input: JsSchemaValueTree
  ): Future[Invocation[Option[JsSchemaValueTree]]]
}
