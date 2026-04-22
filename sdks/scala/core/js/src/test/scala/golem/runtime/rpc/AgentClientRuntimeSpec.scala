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

import golem.data.GolemSchema
import golem.host.js._
import golem.runtime.annotations.{DurabilityMode, agentDefinition}
import golem.BaseAgent
import golem.runtime.{AgentMethod, AgentType, MethodInvocation}
import golem.runtime.rpc.AgentClientRuntime.TestHooks
import golem.runtime.rpc.AgentClientRuntimeSpecFixtures._
import golem.runtime.rpc.host.AgentHostApi
import zio._
import zio.test._
import zio.blocks.schema.Schema

import scala.collection.mutable
import scala.concurrent.Future
import scala.scalajs.js

object AgentClientRuntimeSpec extends ZIOSpecDefault {

  def spec = suite("AgentClientRuntimeSpec")(
    test("ResolvedAgent encodes awaitable inputs and decodes results") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)

      val method   = findMethod[RpcParityAgent, SampleInput, SampleOutput](agentType, "rpcCall")
      val expected = SampleOutput("ack")
      invoker.enqueueInvokeResult(encodeValue(expected)(using method.outputSchema))

      ZIO.fromFuture(_ => resolved.call(method, SampleInput("hello", 2))).map { result =>
        assertTrue(
          result == expected,
          invoker.invokeCalls.headOption.exists(_._1 == method.functionName)
        )
      }
    },
    test("ResolvedAgent routes fire-and-forget calls through trigger and schedule") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)
      val method    = findMethod[RpcParityAgent, String, Unit](agentType, "fireAndForget")

      val triggerF  = resolved.trigger(method, "event")
      val scheduleF = resolved.schedule(method, golem.Datetime.fromEpochMillis(42), "event")

      ZIO.fromFuture(implicit ec => triggerF.flatMap(_ => scheduleF)).map { _ =>
        assertTrue(
          invoker.triggerCalls.headOption.exists(_._1 == method.functionName),
          invoker.scheduleCalls.nonEmpty
        )
      }
    },
    test("Awaitable calls reject when RPC invoker returns an error") {
      val invoker = new RecordingRpcInvoker
      invoker.enqueueInvokeResult(Left("rpc failed"))
      val resolved = resolvedAgent(invoker)
      val method   = findMethod[RpcParityAgent, SampleInput, SampleOutput](rpcAgentType, "rpcCall")

      ZIO.fromFuture(_ => resolved.call(method, SampleInput("oops", 1))).flip.map { ex =>
        assertTrue(ex.getMessage.contains("rpc failed"))
      }
    },
    test("Trigger works for awaitable methods (invocation kind does not restrict trigger)") {
      val agentType                                             = rpcAgentType
      val invoker                                               = new RecordingRpcInvoker
      val resolved                                              = resolvedAgent(invoker, agentType)
      val methodBase: AgentMethod[RpcParityAgent, String, Unit] =
        findMethod[RpcParityAgent, String, Unit](agentType, "fireAndForget")
      val method: AgentMethod[RpcParityAgent, String, Unit] =
        methodBase.copy(invocation = MethodInvocation.Awaitable)

      ZIO.fromFuture(_ => resolved.trigger(method, "noop")).map { _ =>
        assertTrue(
          invoker.triggerCalls.nonEmpty,
          invoker.triggerCalls.head._1 == method.functionName
        )
      }
    },
    test("Schedule works for awaitable methods (invocation kind does not restrict schedule)") {
      val agentType                                             = rpcAgentType
      val invoker                                               = new RecordingRpcInvoker
      val resolved                                              = resolvedAgent(invoker, agentType)
      val methodBase: AgentMethod[RpcParityAgent, String, Unit] =
        findMethod[RpcParityAgent, String, Unit](agentType, "fireAndForget")
      val method: AgentMethod[RpcParityAgent, String, Unit] =
        methodBase.copy(invocation = MethodInvocation.Awaitable)

      ZIO.fromFuture(_ => resolved.schedule(method, golem.Datetime.fromEpochMillis(1.0), "noop")).map { _ =>
        assertTrue(
          invoker.scheduleCalls.nonEmpty,
          invoker.scheduleCalls.head._2 == method.functionName
        )
      }
    },
    test("AgentClient binder override proxies awaitable RPC methods") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)

      TestHooks.withClientBinder(manualBinder(agentType)) {
        val client =
          TestHooks.bindOverride(resolved).getOrElse(throw new RuntimeException("client binder override missing"))

        val output = SampleOutput("ack")
        invoker.enqueueInvokeResult(encodeValue(output))

        val method = findMethod[RpcParityAgent, SampleInput, SampleOutput](agentType, "rpcCall")
        ZIO.fromFuture(_ => client.rpcCall(SampleInput("hello", 1))).map { result =>
          assertTrue(
            result == output,
            invoker.invokeCalls.headOption.exists(_._1 == method.functionName)
          )
        }
      }
    },
    test("AgentClient binder override proxies fire-and-forget RPC methods") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)

      TestHooks.withClientBinder(manualBinder(agentType)) {
        val client =
          TestHooks.bindOverride(resolved).getOrElse(throw new RuntimeException("client binder override missing"))
        val method = findMethod[RpcParityAgent, String, Unit](agentType, "fireAndForget")
        client.fireAndForget("event")
        assertTrue(
          invoker.triggerCalls.nonEmpty,
          invoker.triggerCalls.head._1 == method.functionName
        )
      }
    }
  )

  private def rpcAgentType: AgentType[RpcParityAgent, Any] =
    AgentClient.agentType[RpcParityAgent].asInstanceOf[AgentType[RpcParityAgent, Any]]

  private def resolvedAgent(
    invoker: RecordingRpcInvoker,
    agentType: AgentType[RpcParityAgent, Any] = rpcAgentType
  ) =
    AgentClientRuntime.ResolvedAgent(
      agentType,
      stubRemote(agentType, invoker)
    )

  private def stubRemote(
    agentType: AgentType[RpcParityAgent, Any],
    invoker: RpcInvoker
  ): RemoteAgentClient = {
    val metadata = js.Dynamic
      .literal(
        "agent-type"     -> js.Dynamic.literal("type-name" -> agentType.typeName),
        "implemented-by" -> js.Dynamic.literal(
          "uuid" -> js.Dynamic.literal("high-bits" -> 0.0, "low-bits" -> 0.0)
        )
      )
      .asInstanceOf[AgentHostApi.RegisteredAgentType]
    RemoteAgentClient(agentType.typeName, "agent-1", metadata, invoker)
  }

  private def encodeValue[A](value: A)(implicit codec: GolemSchema[A]): Either[String, JsDataValue] =
    RpcValueCodec.encodeValue(value).map(wv => JsDataValue.tuple(js.Array(JsElementValue.componentModel(wv))))

  private def manualBinder(
    rpcAgentType: AgentType[RpcParityAgent, Any]
  ): AgentClientRuntime.ResolvedAgent[RpcParityAgent] => RpcParityAgent =
    resolved =>
      new RpcParityAgent {
        override def `new`(token: String): RpcParityAgent =
          this

        override def rpcCall(input: SampleInput): Future[SampleOutput] = {
          val method = findMethod[RpcParityAgent, SampleInput, SampleOutput](rpcAgentType, "rpcCall")
          resolved.call(method, input)
        }

        override def multiArgs(message: String, count: Int): Future[Int] = {
          val method = findMethod[RpcParityAgent, Vector[Any], Int](rpcAgentType, "multiArgs")
          resolved.call(method, Vector[Any](message, count))
        }

        override def fireAndForget(event: String): Unit = {
          val method = findMethod[RpcParityAgent, String, Unit](rpcAgentType, "fireAndForget")
          resolved
            .trigger(method, event)
            .failed
            .foreach { err =>
              js.Dynamic.global.console.error("fire-and-forget trigger failed", err.asInstanceOf[js.Any])
            }(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)
          ()
        }
      }

  private def findMethod[Trait, In, Out](
    agentType: AgentType[Trait, Any],
    name: String
  ): AgentMethod[Trait, In, Out] =
    agentType.methods.collectFirst {
      case candidate if candidate.metadata.name == name =>
        candidate.asInstanceOf[AgentMethod[Trait, In, Out]]
    }.getOrElse(throw new IllegalArgumentException(s"Method definition for $name not found"))

  private final class RecordingRpcInvoker extends RpcInvoker {
    val invokeCalls   = mutable.ListBuffer.empty[(String, JsDataValue)]
    val triggerCalls  = mutable.ListBuffer.empty[(String, JsDataValue)]
    val scheduleCalls = mutable.ListBuffer.empty[(golem.Datetime, String, JsDataValue)]

    private val invokeResults = mutable.Queue.empty[Either[String, JsDataValue]]

    def enqueueInvokeResult(result: Either[String, JsDataValue]): Unit =
      invokeResults.enqueue(result)

    override def invokeAndAwait(functionName: String, input: JsDataValue): Either[String, JsDataValue] = {
      invokeCalls += ((functionName, input))
      if (invokeResults.nonEmpty) invokeResults.dequeue()
      else Right(js.Dynamic.literal().asInstanceOf[JsDataValue])
    }

    override def invoke(functionName: String, input: JsDataValue): Either[String, Unit] = {
      triggerCalls += ((functionName, input))
      Right(())
    }

    override def scheduleInvocation(
      datetime: golem.Datetime,
      functionName: String,
      input: JsDataValue
    ): Either[String, Unit] = {
      scheduleCalls += ((datetime, functionName, input))
      Right(())
    }

    override def scheduleCancelableInvocation(
      datetime: golem.Datetime,
      functionName: String,
      input: JsDataValue
    ): Either[String, CancellationToken] =
      Left("not used")
  }
}

private object AgentClientRuntimeSpecFixtures {
  @agentDefinition(mode = DurabilityMode.Durable)
  trait RpcParityAgent extends BaseAgent {
    class Id(val token: String)

    def `new`(token: String): RpcParityAgent

    def rpcCall(input: SampleInput): Future[SampleOutput]

    def multiArgs(message: String, count: Int): Future[Int]

    def fireAndForget(event: String): Unit
  }

  final case class RpcCtor(token: String)

  final case class SampleInput(message: String, count: Int)

  final case class SampleOutput(result: String)

  object RpcCtor {
    implicit val schemaRpcCtor: Schema[RpcCtor] = Schema.derived
  }

  object SampleInput {
    implicit val schemaSampleInput: Schema[SampleInput] = Schema.derived
  }

  object SampleOutput {
    implicit val schemaSampleOutput: Schema[SampleOutput] = Schema.derived
  }
}
