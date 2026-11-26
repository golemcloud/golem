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

  private val metadata = {
    val agentId = golem.AgentId(java.util.UUID.fromString("0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d"))
    InvocationMetadata(
      agentId,
      golem.InvocationId(agentId, java.util.UUID.fromString("3f2a1b0c-9d8e-4f7a-b6c5-d4e3f2a1b0c9"))
    )
  }

  def spec = suite("AgentClientTypeEndToEndSpec")(
    test("client type loopback via AgentClientRuntime.resolve (Future-returning method)") {
      ZIO.fromFuture { implicit ec =>
        val _ = asyncEchoDefn

        val agentType = golem.runtime.macros.AgentClientMacro.agentType[AsyncEchoAgent]

        // The host loopback returns the single output `some(tree)` for "echo".
        val rpc = new RpcInvoker {
          override def invokeAndAwait(
            functionName: String,
            input: JsSchemaValueTree,
            scopeCard: Option[golem.host.PermissionsApi.ScopeCard]
          ): scala.concurrent.Future[InvocationResult[Option[JsSchemaValueTree]]] =
            if (functionName != "echo")
              scala.concurrent.Future
                .failed(AgentRpcException(AgentRpcError.NotFound(s"unexpected method: $functionName")))
            else
              scala.concurrent.Future.successful(
                InvocationResult(metadata, SchemaRpcCodec.encodeSingleResult("hello world"))
              )

          override def trigger(
            @unused functionName: String,
            @unused input: JsSchemaValueTree
          ): scala.concurrent.Future[Invocation[Option[JsSchemaValueTree]]] =
            scala.concurrent.Future.failed(new UnsupportedOperationException("not used"))

          override def schedule(
            @unused datetime: golem.Datetime,
            @unused functionName: String,
            @unused input: JsSchemaValueTree
          ): scala.concurrent.Future[Invocation[Option[JsSchemaValueTree]]] =
            scala.concurrent.Future.failed(new UnsupportedOperationException("not used"))
        }

        val resolvedAgent =
          AgentClientRuntime.ResolvedAgent(
            agentType.asInstanceOf[golem.runtime.AgentType[AsyncEchoAgent, Any]],
            RemoteAgentClient("e2e-client-async", rpc)
          )

        val echo = agentType.methods.collectFirst { case m: AgentMethod[AsyncEchoAgent, String, String] @unchecked =>
          m
        }.get

        resolvedAgent.await(echo, "world")
      }.map(out => assertTrue(out == "hello world"))
    }
  )
}
