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

import golem.host.js._
import golem.BaseAgent
import golem.runtime.AgentMethod
import golem.runtime.annotations.{DurabilityMode, agentDefinition, agentImplementation}
import golem.runtime.autowire.AgentImplementation
import golem.runtime.rpc.RpcValueCodec
import zio._
import zio.test._

import scala.annotation.unused
import scala.concurrent.Future
import scala.scalajs.js

object AgentClientTypeEndToEndSpec extends ZIOSpecDefault {

  @agentDefinition("E2eClientAsync", mode = DurabilityMode.Durable)
  trait AsyncEchoAgent extends BaseAgent {
    class Id()
    def echo(in: String): Future[String]
  }

  @agentImplementation()
  final class AsyncEchoAgentImpl() extends AsyncEchoAgent {
    override def echo(in: String): Future[String] =
      Future.successful(s"hello $in")
  }

  private lazy val asyncEchoDefn = AgentImplementation.registerClass[AsyncEchoAgent, AsyncEchoAgentImpl]

  def spec = suite("AgentClientTypeEndToEndSpec")(
    test("client type loopback via AgentClientRuntime.resolve (Future-returning method)") {
      ZIO.fromFuture { implicit ec =>
        val _ = asyncEchoDefn

        val agentType = golem.runtime.macros.AgentClientMacro.agentType[AsyncEchoAgent]

        val rpc = new RpcInvoker {
          override def invokeAndAwait(functionName: String, input: JsDataValue): Either[String, JsDataValue] =
            if (functionName != "echo") Left(s"unexpected method: $functionName")
            else {
              import golem.GolemSchema._
              val witValue = RpcValueCodec.encodeValue("hello world")
              witValue.map { wv =>
                JsDataValue.tuple(js.Array(JsElementValue.componentModel(wv)))
              }
            }

          override def asyncInvokeAndAwait(functionName: String, input: JsDataValue): scala.concurrent.Future[JsDataValue] =
            if (functionName != "echo") scala.concurrent.Future.failed(js.JavaScriptException(s"unexpected method: $functionName"))
            else {
              import golem.GolemSchema._
              val witValue = RpcValueCodec.encodeValue("hello world")
              witValue match {
                case Right(wv) => scala.concurrent.Future.successful(JsDataValue.tuple(js.Array(JsElementValue.componentModel(wv))))
                case Left(err) => scala.concurrent.Future.failed(js.JavaScriptException(err))
              }
            }

          override def cancelableAsyncInvokeAndAwait(functionName: String, input: JsDataValue): (scala.concurrent.Future[JsDataValue], CancellationToken) =
            (asyncInvokeAndAwait(functionName, input), CancellationToken.fromFunction(() => ()))

          override def invoke(functionName: String, input: JsDataValue): Either[String, Unit] =
            Left("not used")

          override def scheduleInvocation(
            @unused datetime: golem.Datetime,
            @unused functionName: String,
            @unused input: JsDataValue
          ): Either[String, Unit] =
            Left("not used")

          override def scheduleCancelableInvocation(
            @unused datetime: golem.Datetime,
            @unused functionName: String,
            @unused input: JsDataValue
          ): Either[String, CancellationToken] =
            Left("not used")
        }

        val resolvedAgent =
          AgentClientRuntime.ResolvedAgent(
            agentType.asInstanceOf[golem.runtime.AgentType[AsyncEchoAgent, Any]],
            RemoteAgentClient("e2e-client-async", "fake-id", null, rpc)
          )

        val echo = agentType.methods.collectFirst { case m: AgentMethod[AsyncEchoAgent, String, String] @unchecked =>
          m
        }.get

        resolvedAgent.await(echo, "world")
      }.map(out => assertTrue(out == "hello world"))
    }
  )
}
