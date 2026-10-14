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

import golem.host.PermissionsApi.{PermissionCard, ScopeCard}
import golem.host.js.JsPermissionCardResource
import golem.host.js.schema.JsSchemaValueTree
import golem.runtime.annotations.{DurabilityMode, agentDefinition}
import golem.BaseAgent
import golem.runtime.{AgentMethod, AgentType}
import golem.runtime.rpc.AgentClientRuntime.WireResolvedAgent
import golem.runtime.rpc.AgentClientRuntimeSpecFixtures._
import golem.schema.{AgentStream, GuestPermissionCardHandle, IntoSchema}
import golem.schema.wire.ConcreteCodec
import zio._
import zio.test._
import zio.blocks.schema.Schema

import scala.collection.mutable
import scala.concurrent.{ExecutionContext, Future}
import scala.scalajs.js

object AgentClientRuntimeSpec extends ZIOSpecDefault {

  def spec = suite("AgentClientRuntimeSpec")(
    test("ResolvedAgent encodes awaitable inputs and decodes results") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)

      val method   = findMethod[RpcParityAgent, SampleInput, SampleOutput](agentType, "rpcCall")
      val expected = SampleOutput("ack")
      invoker.enqueueInvokeResult(Right(encodeResult(expected)))

      ZIO.fromFuture(_ => resolved.await(method, SampleInput("hello", 2))).map { result =>
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
    test("awaited calls carry the default scope card, a per-call card overrides it, and the card stays usable") {
      val invoker   = new RecordingRpcInvoker
      val wire      = WireResolvedAgent(rpcWireType, RemoteAgentClient("ScopeWireAgent", "agent-1", null, invoker))
      val method    = wire.methodByName[SampleInput, SampleOutput]("rpcCall")
      val default   = scopeCard("default")
      val perCall   = scopeCard("per-call")
      val defaulted = wire.withScopeCard(default)
      invoker.enqueueInvokeResult(Right(encodeResult(SampleOutput("a"))))
      invoker.enqueueInvokeResult(Right(encodeResult(SampleOutput("b"))))
      invoker.enqueueInvokeResult(Right(encodeResult(SampleOutput("c"))))
      invoker.enqueueInvokeResult(Right(encodeResult(SampleOutput("d"))))
      ZIO.fromFuture { implicit ec =>
        for {
          _ <- defaulted.await(method, SampleInput("x", 1))
          _ <- defaulted.withScopeCard(perCall).await(method, SampleInput("x", 2))
          _ <- defaulted.withoutScopeCard.await(method, SampleInput("x", 3))
          _ <- defaulted.cancelableAwait(method, SampleInput("x", 4))._1
        } yield assertTrue(
          invoker.scopeCardsSeen.toList.map(_.map(_.toString)) ==
            List(Some(default.toString), Some(perCall.toString), None, Some(default.toString)),
          invoker.scopeCardsSeen(0).exists(_ eq default),
          invoker.scopeCardsSeen(1).exists(_ eq perCall),
          invoker.scopeCardsSeen(3).exists(_ eq default),
          default.isPresent,
          perCall.isPresent,
          wire.scopeCard.isEmpty
        )
      }
    },
    test("fire-and-forget and scheduled calls never carry the default scope card") {
      val invoker = new RecordingRpcInvoker
      val wire    = WireResolvedAgent(rpcWireType, RemoteAgentClient("ScopeWireAgent", "agent-1", null, invoker))
        .withScopeCard(scopeCard("d"))
      val method = wire.methodByName[String, Unit]("fireAndForget")
      ZIO.fromFuture { implicit ec =>
        for {
          _ <- wire.trigger(method, "event")
          _ <- wire.schedule(method, golem.Datetime.fromEpochMillis(1.0), "event")
        } yield assertTrue(
          invoker.triggerCalls.size == 1,
          invoker.scheduleCalls.size == 1,
          invoker.scopeCardsSeen.isEmpty
        )
      }
    },
    test("a scope card moved before the call fails it without consuming an argument card") {
      val invoker = new RecordingRpcInvoker
      val scope   = scopeCard("moved-before")
      val wire    = WireResolvedAgent(rpcWireType, RemoteAgentClient("ScopeWireAgent", "agent-1", null, invoker))
        .withScopeCard(scope)
      val method   = wire.methodByName[PermissionCard, Boolean]("sendCard")
      val argument = permissionCard("argument")
      scope.release()
      ZIO.fromFuture(_ => wire.await(method, argument)).flip.map { error =>
        assertTrue(
          error.isInstanceOf[IllegalStateException],
          argument.isPresent,
          invoker.invokeCalls.isEmpty
        )
      }
    },
    test("a scope card moved while the input is lowered gives the argument card back") {
      val invoker = new RecordingRpcInvoker
      val scope   = scopeCard("moved-during")
      val wire    = WireResolvedAgent(rpcWireType, RemoteAgentClient("ScopeWireAgent", "agent-1", null, invoker))
        .withScopeCard(scope)
      val method   = wire.methodByName[PermissionCard, Boolean]("sendCard")
      val raw      = scala.scalajs.js.Dynamic.literal(name = "argument")
      val argument = PermissionCard.fromRaw(raw.asInstanceOf[JsPermissionCardResource])
      val call     = wire.await(method, argument)
      scope.release()
      ZIO.fromFuture(_ => call).flip.map { error =>
        assertTrue(
          error.isInstanceOf[IllegalStateException],
          argument.isPresent,
          argument.borrowRaw(_ eq raw),
          invoker.invokeCalls.isEmpty
        )
      }
    },
    test("ResolvedAgent attaches the scope card to awaited calls only") {
      val invoker  = new RecordingRpcInvoker
      val card     = scopeCard("reflective")
      val resolved = resolvedAgent(invoker).withScopeCard(card)
      val method   = findMethod[RpcParityAgent, SampleInput, SampleOutput](rpcAgentType, "rpcCall")
      val trigger  = findMethod[RpcParityAgent, String, Unit](rpcAgentType, "fireAndForget")
      invoker.enqueueInvokeResult(Right(encodeResult(SampleOutput("ok"))))
      ZIO.fromFuture { implicit ec =>
        for {
          _ <- resolved.await(method, SampleInput("x", 1))
          _ <- resolved.trigger(trigger, "event")
        } yield assertTrue(
          invoker.scopeCardsSeen.toList.map(_.exists(_ eq card)) == List(true),
          invoker.triggerCalls.size == 1,
          card.isPresent
        )
      }
    },
    test("Awaitable calls reject when RPC invoker returns an error") {
      val invoker = new RecordingRpcInvoker
      invoker.enqueueInvokeResult(Left("rpc failed"))
      val resolved = resolvedAgent(invoker)
      val method   = findMethod[RpcParityAgent, SampleInput, SampleOutput](rpcAgentType, "rpcCall")

      ZIO.fromFuture(_ => resolved.await(method, SampleInput("oops", 1))).flip.map { ex =>
        assertTrue(ex.getMessage.contains("rpc failed"))
      }
    },
    test("Trigger works for any method") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)
      val method    = findMethod[RpcParityAgent, String, Unit](agentType, "fireAndForget")

      ZIO.fromFuture(_ => resolved.trigger(method, "noop")).map { _ =>
        assertTrue(
          invoker.triggerCalls.nonEmpty,
          invoker.triggerCalls.head._1 == method.functionName
        )
      }
    },
    test("Schedule works for any method") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)
      val method    = findMethod[RpcParityAgent, String, Unit](agentType, "fireAndForget")

      ZIO.fromFuture(_ => resolved.schedule(method, golem.Datetime.fromEpochMillis(1.0), "noop")).map { _ =>
        assertTrue(
          invoker.scheduleCalls.nonEmpty,
          invoker.scheduleCalls.head._2 == method.functionName
        )
      }
    },
    test("trigger rejects streams without consuming them") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)
      val method    = findMethod[RpcParityAgent, AgentStream[String], Unit](agentType, "consumeStream")
      val stream    = AgentStream.fromPull(() => Future.successful(Some("still-owned")))

      ZIO.fromFuture { implicit ec =>
        resolved.trigger(method, stream).failed.flatMap { _ =>
          stream.pull().map { remaining =>
            assertTrue(
              remaining.contains("still-owned"),
              invoker.triggerCalls.isEmpty
            )
          }
        }
      }
    },
    test("wire immediate encoding rejects a live stream before transferring ownership") {
      var closed = 0
      val stream = AgentStream.fromPull[String](
        () => Future.successful(Some("still-owned")),
        () => { closed += 1; Future.successful(()) }
      )
      val encoded = AgentClientRuntime.TestHooks.encodeImmediate(
        ConcreteCodec.derived[AgentStream[String]],
        containsStream = true,
        stream
      )

      ZIO.fromFuture(_ => stream.pull()).map { remaining =>
        assertTrue(
          encoded.left.exists(_.contains("live streams cannot cross")),
          remaining.contains("still-owned"),
          closed == 0
        )
      }
    }
  )

  private def rpcWireType: golem.runtime.WireAgentClientType[ScopeWireAgent, ?] =
    golem.runtime.macros.AgentClientMacro.wireType[ScopeWireAgent]

  private def scopeCard(name: String): ScopeCard =
    new ScopeCard(GuestPermissionCardHandle.fromRaw(scala.scalajs.js.Dynamic.literal(name = name)))

  private def permissionCard(name: String): PermissionCard =
    PermissionCard.fromRaw(scala.scalajs.js.Dynamic.literal(name = name).asInstanceOf[JsPermissionCardResource])

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
  ): RemoteAgentClient =
    // The `metadata` field is not read on the awaitable / fire-and-forget path.
    RemoteAgentClient(agentType.typeName, "agent-1", null, invoker)

  /** Encode a single-output method result as the host `some(tree)`. */
  private def encodeResult[A](value: A)(implicit into: IntoSchema[A]): Option[JsSchemaValueTree] =
    SchemaRpcCodec.encodeSingleResult(value)

  private def findMethod[Trait, In, Out](
    agentType: AgentType[Trait, Any],
    name: String
  ): AgentMethod[Trait, In, Out] =
    agentType.methods.collectFirst {
      case candidate if candidate.metadata.name == name =>
        candidate.asInstanceOf[AgentMethod[Trait, In, Out]]
    }.getOrElse(throw new IllegalArgumentException(s"Method definition for $name not found"))

  private final class RecordingRpcInvoker extends RpcInvoker {
    val invokeCalls    = mutable.ListBuffer.empty[(String, JsSchemaValueTree)]
    val scopeCardsSeen = mutable.ListBuffer.empty[Option[ScopeCard]]
    val triggerCalls   = mutable.ListBuffer.empty[(String, JsSchemaValueTree)]
    val scheduleCalls  = mutable.ListBuffer.empty[(golem.Datetime, String, JsSchemaValueTree)]

    private val invokeResults = mutable.Queue.empty[Either[String, Option[JsSchemaValueTree]]]

    def enqueueInvokeResult(result: Either[String, Option[JsSchemaValueTree]]): Unit =
      invokeResults.enqueue(result)

    override def invokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree,
      scopeCard: Option[ScopeCard]
    ): Either[String, Option[JsSchemaValueTree]] = {
      invokeCalls += ((functionName, input))
      scopeCardsSeen += scopeCard
      if (invokeResults.nonEmpty) invokeResults.dequeue()
      else Right(None)
    }

    override def asyncInvokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree,
      scopeCard: Option[ScopeCard]
    ): Future[Option[JsSchemaValueTree]] = {
      invokeCalls += ((functionName, input))
      scopeCardsSeen += scopeCard
      if (invokeResults.nonEmpty) {
        invokeResults.dequeue() match {
          case Right(v)  => Future.successful(v)
          case Left(err) => Future.failed(js.JavaScriptException(err))
        }
      } else Future.successful(None)
    }

    override def cancelableAsyncInvokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree,
      scopeCard: Option[ScopeCard]
    ): (Future[Option[JsSchemaValueTree]], CancellationToken) =
      (asyncInvokeAndAwait(functionName, input, scopeCard), CancellationToken.fromFunction(() => ()))

    override def invoke(functionName: String, input: JsSchemaValueTree): Either[String, Unit] = {
      triggerCalls += ((functionName, input))
      Right(())
    }

    override def scheduleInvocation(
      datetime: golem.Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Either[String, Unit] = {
      scheduleCalls += ((datetime, functionName, input))
      Right(())
    }

    override def scheduleCancelableInvocation(
      datetime: golem.Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Either[String, CancellationToken] =
      Left("not used")
  }
}

private object AgentClientRuntimeSpecFixtures {
  implicit val agentStreamExecutionContext: ExecutionContext =
    scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

  @agentDefinition(mode = DurabilityMode.Durable)
  trait RpcParityAgent extends BaseAgent {
    class Id(val token: String)

    def `new`(token: String): RpcParityAgent

    def rpcCall(input: SampleInput): Future[SampleOutput]

    def multiArgs(message: String, count: Int): Future[Int]

    def fireAndForget(event: String): Unit

    def consumeStream(stream: AgentStream[String]): Unit
  }

  @agentDefinition(mode = DurabilityMode.Durable)
  trait ScopeWireAgent extends BaseAgent {
    class Id(val token: String)

    def rpcCall(input: SampleInput): Future[SampleOutput]

    def fireAndForget(event: String): Unit

    def sendCard(card: PermissionCard): Future[Boolean]
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
