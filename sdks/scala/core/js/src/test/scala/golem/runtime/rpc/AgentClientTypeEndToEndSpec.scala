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

import golem.host.js.schema.JsSchemaValueTree
import golem.BaseAgent
import golem.runtime.AgentMethod
import golem.runtime.annotations.{DurabilityMode, agentDefinition, agentImplementation}
import golem.runtime.autowire.AgentImplementation
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

        // The host loopback returns the single output `some(tree)` for "echo".
        val rpc = new RpcInvoker {
          override def invokeAndAwait(
            functionName: String,
            input: JsSchemaValueTree
          ): Either[String, Option[JsSchemaValueTree]] =
            if (functionName != "echo") Left(s"unexpected method: $functionName")
            else Right(SchemaRpcCodec.encodeSingleResult("hello world"))

          override def asyncInvokeAndAwait(
            functionName: String,
            input: JsSchemaValueTree
          ): scala.concurrent.Future[Option[JsSchemaValueTree]] =
            if (functionName != "echo")
              scala.concurrent.Future.failed(js.JavaScriptException(s"unexpected method: $functionName"))
            else scala.concurrent.Future.successful(SchemaRpcCodec.encodeSingleResult("hello world"))

          override def cancelableAsyncInvokeAndAwait(
            functionName: String,
            input: JsSchemaValueTree
          ): (scala.concurrent.Future[Option[JsSchemaValueTree]], CancellationToken) =
            (asyncInvokeAndAwait(functionName, input), CancellationToken.fromFunction(() => ()))

          override def invoke(functionName: String, input: JsSchemaValueTree): Either[String, Unit] =
            Left("not used")

          override def scheduleInvocation(
            @unused datetime: golem.Datetime,
            @unused functionName: String,
            @unused input: JsSchemaValueTree
          ): Either[String, Unit] =
            Left("not used")

          override def scheduleCancelableInvocation(
            @unused datetime: golem.Datetime,
            @unused functionName: String,
            @unused input: JsSchemaValueTree
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
