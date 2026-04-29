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

import golem.Datetime
import golem.Uuid
import golem.config.{ConfigOverride, ConfigOverrideEncoder}
import golem.FutureInterop
import golem.host.js._
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
)

object RemoteAgentClient {
  def resolve(agentTypeName: String, constructorPayload: JsDataValue): Either[String, RemoteAgentClient] =
    resolve(agentTypeName, constructorPayload, phantom = None)

  def resolve(
    agentTypeName: String,
    constructorPayload: JsDataValue,
    phantom: Option[Uuid]
  ): Either[String, RemoteAgentClient] =
    resolve(agentTypeName, constructorPayload, phantom, configOverrides = Nil)

  def resolve(
    agentTypeName: String,
    constructorPayload: JsDataValue,
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
    override def invokeAndAwait(functionName: String, input: JsDataValue): Either[String, JsDataValue] =
      invokeWithFallback(functionName)(fn => client.invokeAndAwait(fn, input).left.map(_.toString))

    override def asyncInvokeAndAwait(functionName: String, input: JsDataValue): Future[JsDataValue] =
      safeCall(client.asyncInvokeAndAwait(functionName, input).left.map(_.toString)) match {
        case Left(err) => Future.failed(JavaScriptException(err))
        case Right(futureResult) =>
          val pollable = futureResult.subscribe()
          FutureInterop.fromPromise(pollable.promise()).flatMap { _ =>
            futureResult.get().toOption match {
              case Some(result) =>
                if (result.tag == "ok") {
                  Future.successful(result.asInstanceOf[JsOk[JsDataValue]].value)
                } else {
                  val rpcError = result.asInstanceOf[JsErr[JsRpcError]].value
                  Future.failed(JavaScriptException(rpcError.toString))
                }
              case None =>
                Future.failed(JavaScriptException("async RPC: pollable ready but no result available"))
            }
          }(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)
      }

    override def cancelableAsyncInvokeAndAwait(functionName: String, input: JsDataValue): (Future[JsDataValue], CancellationToken) =
      safeCall(client.asyncInvokeAndAwait(functionName, input).left.map(_.toString)) match {
        case Left(err) =>
          (Future.failed(JavaScriptException(err)), CancellationToken.fromFunction(() => ()))
        case Right(futureResult) =>
          val token = CancellationToken.fromFunction(() => futureResult.cancel())
          val future = {
            val pollable = futureResult.subscribe()
            FutureInterop.fromPromise(pollable.promise()).flatMap { _ =>
              futureResult.get().toOption match {
                case Some(result) =>
                  if (result.tag == "ok") {
                    Future.successful(result.asInstanceOf[JsOk[JsDataValue]].value)
                  } else {
                    val rpcError = result.asInstanceOf[JsErr[JsRpcError]].value
                    Future.failed(JavaScriptException(rpcError.toString))
                  }
                case None =>
                  Future.failed(JavaScriptException("async RPC: pollable ready but no result available"))
              }
            }(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)
          }
          (future, token)
      }

    override def invoke(functionName: String, input: JsDataValue): Either[String, Unit] =
      invokeWithFallback(functionName)(fn => client.invoke(fn, input).left.map(_.toString))

    override def scheduleInvocation(
      datetime: Datetime,
      functionName: String,
      input: JsDataValue
    ): Either[String, Unit] =
      invokeWithFallback(functionName)(fn => client.scheduleInvocation(datetime, fn, input).left.map(_.toString))

    override def scheduleCancelableInvocation(
      datetime: Datetime,
      functionName: String,
      input: JsDataValue
    ): Either[String, CancellationToken] =
      invokeWithFallback(functionName)(fn => client.scheduleCancelableInvocation(datetime, fn, input).left.map(_.toString))
  }

  private def invokeWithFallback[A](functionName: String)(call: String => Either[String, A]): Either[String, A] =
    safeCall(call(functionName))

  private def safeCall[A](thunk: => Either[String, A]): Either[String, A] =
    try thunk
    catch {
      case JavaScriptException(err) => Left(err.toString)
    }

}
