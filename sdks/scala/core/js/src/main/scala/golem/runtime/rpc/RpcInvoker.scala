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
import golem.host.js.schema.JsSchemaValueTree

import scala.concurrent.Future

/**
 * The `golem:agent/host@2.0.0` RPC client surface. Method inputs are a single
 * `schema-value-tree` (the parameter-list record) and awaited results are an
 * `option<schema-value-tree>` (modelled as a Scala [[Option]]): `None` for a
 * `unit` output, `Some(tree)` for a `single` output.
 */
private[rpc] trait RpcInvoker {
  def invokeAndAwait(functionName: String, input: JsSchemaValueTree): Either[String, Option[JsSchemaValueTree]]

  def invokeAndAwaitWithMetadata(
    functionName: String,
    input: JsSchemaValueTree
  ): Either[String, InvocationResult[Option[JsSchemaValueTree]]] = Left("invocation metadata is not supported")

  def asyncInvokeAndAwait(functionName: String, input: JsSchemaValueTree): Future[Option[JsSchemaValueTree]]

  def asyncInvokeAndAwaitWithMetadata(
    functionName: String,
    input: JsSchemaValueTree
  ): Either[String, AsyncInvocation[Option[JsSchemaValueTree]]] = Left("invocation metadata is not supported")

  def cancelableAsyncInvokeAndAwait(
    functionName: String,
    input: JsSchemaValueTree
  ): (Future[Option[JsSchemaValueTree]], CancellationToken)

  def invoke(functionName: String, input: JsSchemaValueTree): Either[String, Unit]

  def invokeWithMetadata(functionName: String, input: JsSchemaValueTree): Either[String, InvocationMetadata] =
    Left("invocation metadata is not supported")

  def scheduleInvocation(datetime: Datetime, functionName: String, input: JsSchemaValueTree): Either[String, Unit]

  def scheduleInvocationWithMetadata(
    datetime: Datetime,
    functionName: String,
    input: JsSchemaValueTree
  ): Either[String, InvocationReceipt] = Left("invocation metadata is not supported")

  def scheduleCancelableInvocation(
    datetime: Datetime,
    functionName: String,
    input: JsSchemaValueTree
  ): Either[String, CancellationToken]

  def scheduleCancelableInvocationWithMetadata(
    datetime: Datetime,
    functionName: String,
    input: JsSchemaValueTree
  ): Either[String, CancelableInvocationReceipt] = Left("invocation metadata is not supported")
}
