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
import golem.host.js.{JsErr, JsOk}
import golem.host.js.schema.{JsSchemaValueTree, JsTypedAgentConfigValue, JsUuid}
import golem.runtime.rpc.host.AgentHostApi.RegisteredAgentType
import golem.runtime.rpc.host.WasmRpcApi.WasmRpcClient
import golem.runtime.rpc.host.{AgentHostApi, JsRpcError, WasmRpcApi}

import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.JavaScriptException

final case class RemoteAgentClient(
  agentTypeName: String,
  agentId: String,
  metadata: RegisteredAgentType,
  rpc: RpcInvoker
)

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

  private final class WasmRpcInvoker(client: WasmRpcClient) extends RpcInvoker {
    override def invokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree
    ): Either[String, Option[JsSchemaValueTree]] =
      invokeWithFallback(functionName)(fn => client.invokeAndAwait(fn, input).map(_.toOption).left.map(_.toString))

    override def asyncInvokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree
    ): Future[Option[JsSchemaValueTree]] =
      safeCall(client.asyncInvokeAndAwait(functionName, input).left.map(_.toString)) match {
        case Left(err)           => Future.failed(JavaScriptException(err))
        case Right(futureResult) => awaitFutureResult(futureResult)
      }

    override def cancelableAsyncInvokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree
    ): (Future[Option[JsSchemaValueTree]], CancellationToken) =
      safeCall(client.asyncInvokeAndAwait(functionName, input).left.map(_.toString)) match {
        case Left(err) =>
          (Future.failed(JavaScriptException(err)), CancellationToken.fromFunction(() => ()))
        case Right(futureResult) =>
          val token = CancellationToken.fromFunction(() => futureResult.cancel())
          (awaitFutureResult(futureResult), token)
      }

    /**
     * Drives a host `future-invoke-result` to completion. The synchronous
     * `subscribe()`/`promise()` calls are guarded so a host/JS interop failure
     * surfaces as a failed `Future` rather than an uncaught `TypeError` that
     * would trap the whole guest invocation; `get()` runs inside the `flatMap`
     * body and is already captured by `Future` semantics.
     */
    private def awaitFutureResult(
      futureResult: WasmRpcApi.RawFutureInvokeResult
    ): Future[Option[JsSchemaValueTree]] =
      try {
        val pollable = futureResult.subscribe()
        FutureInterop
          .fromPromise(pollable.promise())
          .flatMap { _ =>
            readAsyncResult(futureResult.get().toOption)
          }(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)
      } catch {
        case JavaScriptException(e) => Future.failed(JavaScriptException(s"async RPC failed: ${e.toString}"))
      }

    override def invoke(functionName: String, input: JsSchemaValueTree): Either[String, Unit] =
      invokeWithFallback(functionName)(fn => client.invoke(fn, input).left.map(_.toString))

    override def scheduleInvocation(
      datetime: Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Either[String, Unit] =
      invokeWithFallback(functionName)(fn => client.scheduleInvocation(datetime, fn, input).left.map(_.toString))

    override def scheduleCancelableInvocation(
      datetime: Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Either[String, CancellationToken] =
      invokeWithFallback(functionName)(fn =>
        client.scheduleCancelableInvocation(datetime, fn, input).left.map(_.toString)
      )

    private def readAsyncResult(
      result: Option[golem.host.js.JsResult[js.UndefOr[JsSchemaValueTree], JsRpcError]]
    ): Future[Option[JsSchemaValueTree]] =
      result match {
        case Some(res) =>
          if (res.tag == "ok") {
            val okValue = res.asInstanceOf[JsOk[js.UndefOr[JsSchemaValueTree]]].value
            Future.successful(okValue.toOption)
          } else {
            val rpcError = res.asInstanceOf[JsErr[JsRpcError]].value
            Future.failed(JavaScriptException(WasmRpcApi.decodeRpcError(rpcError).toString))
          }
        case None =>
          Future.failed(JavaScriptException("async RPC: pollable ready but no result available"))
      }
  }

  private def invokeWithFallback[A](functionName: String)(call: String => Either[String, A]): Either[String, A] =
    safeCall(call(functionName))

  private def safeCall[A](thunk: => Either[String, A]): Either[String, A] =
    try thunk
    catch {
      case JavaScriptException(err) => Left(err.toString)
    }
}
