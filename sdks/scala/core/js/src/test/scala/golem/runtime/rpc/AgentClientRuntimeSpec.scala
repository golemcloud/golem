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

import golem.host.PermissionsApi.{PermissionCard, PolymorphicPermissionCard, ScopeCard}
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
    test("ResolvedAgent metadata calls surface a rejected call's typed error and pair a result with its metadata") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)
      val method    = findMethod[RpcParityAgent, SampleInput, SampleOutput](agentType, "rpcCall")

      invoker.enqueueInvokeResult(Left(AgentRpcError.Denied("missing permission")))
      invoker.enqueueInvokeResult(Right(encodeResult(SampleOutput("ok"))))

      ZIO.fromFuture { implicit ec =>
        for {
          awaited  <- resolved.awaitWithMetadata(method, SampleInput("a", 1)).failed
          accepted <- resolved.awaitWithMetadata(method, SampleInput("c", 3))
        } yield assertTrue(
          awaited == AgentRpcException(AgentRpcError.Denied("missing permission")),
          accepted == InvocationResult(started, SampleOutput("ok"))
        )
      }
    },
    test("ResolvedAgent trigger and schedule return handles that decode the method output") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)
      val method    = findMethod[RpcParityAgent, SampleInput, SampleOutput](agentType, "rpcCall")
      invoker.handleResult = encodeResult(SampleOutput("later"))

      ZIO.fromFuture { implicit ec =>
        for {
          triggered <- resolved.trigger(method, SampleInput("a", 1))
          scheduled <- resolved.schedule(method, golem.Datetime.fromEpochMillis(42), SampleInput("b", 2))
          result    <- triggered.awaitResult()
          status    <- scheduled.status()
          cancelled <- scheduled.cancel()
        } yield assertTrue(
          triggered.id == started.invocationId,
          scheduled.id == started.invocationId,
          result == SampleOutput("later"),
          status == InvocationStatus.Pending,
          cancelled,
          invoker.triggerCalls.map(_._1) == List(method.functionName),
          invoker.scheduleCalls.map(_._2) == List(method.functionName)
        )
      }
    },
    test("awaited calls carry the default scope card, a per-call card overrides it, and the card stays usable") {
      val invoker   = new RecordingRpcInvoker
      val wire      = WireResolvedAgent(rpcWireType, RemoteAgentClient("ScopeWireAgent", invoker))
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
          _ <- defaulted.awaitWithMetadata(method, SampleInput("x", 4))
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
      val wire    = WireResolvedAgent(rpcWireType, RemoteAgentClient("ScopeWireAgent", invoker))
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
    test("agent metadata declares a template card as polymorphic and a concrete card as not") {
      val metadata                              = rpcWireType.metadata
      def body(index: Int)                      = metadata.schema.typeNodes(index).body
      def cardSpec(index: Int): Option[Boolean] = body(index) match {
        case golem.schema.wire.WitSchemaTypeBody.PermissionCardType(spec) => Some(spec.polymorphic)
        case _                                                            => None
      }
      val sendCard     = metadata.methods.find(_.name == "sendCard").get
      val sendTemplate = metadata.methods.find(_.name == "sendTemplate").get
      assertTrue(
        sendCard.parameters.map(p => cardSpec(p.schema)) == List(Some(false)),
        sendTemplate.parameters.map(p => cardSpec(p.schema)) == List(Some(true)),
        sendTemplate.output.flatMap(cardSpec).contains(true)
      )
    },
    test("a scope card moved before the call fails it without consuming an argument card") {
      val invoker = new RecordingRpcInvoker
      val scope   = scopeCard("moved-before")
      val wire    = WireResolvedAgent(rpcWireType, RemoteAgentClient("ScopeWireAgent", invoker))
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
      val wire    = WireResolvedAgent(rpcWireType, RemoteAgentClient("ScopeWireAgent", invoker))
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
      invoker.enqueueInvokeResult(Left(AgentRpcError.RemoteInternal("rpc failed")))
      val resolved = resolvedAgent(invoker)
      val method   = findMethod[RpcParityAgent, SampleInput, SampleOutput](rpcAgentType, "rpcCall")

      ZIO.fromFuture(_ => resolved.await(method, SampleInput("oops", 1))).flip.map { ex =>
        assertTrue(ex == AgentRpcException(AgentRpcError.RemoteInternal("rpc failed")))
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
        resolved.trigger(method, stream).failed.flatMap { error =>
          stream.pull().map { remaining =>
            assertTrue(
              error match {
                case error: IllegalArgumentException => error.getMessage.contains("live streams")
                case _                               => false
              },
              remaining.contains("still-owned"),
              invoker.triggerCalls.isEmpty
            )
          }
        }
      }
    },
    test("trigger and schedule reject a method whose output is a stream before calling the host") {
      val agentType = rpcAgentType
      val invoker   = new RecordingRpcInvoker
      val resolved  = resolvedAgent(invoker, agentType)
      val method    = findMethod[RpcParityAgent, Unit, AgentStream[String]](agentType, "produceStream")

      def rejected(error: Throwable): Boolean =
        error match {
          case error: IllegalArgumentException => error.getMessage.contains("live streams")
          case _                               => false
        }

      ZIO.fromFuture { implicit ec =>
        for {
          triggered <- resolved.trigger(method, ()).failed
          scheduled <- resolved.schedule(method, golem.Datetime.fromEpochMillis(1.0), ()).failed
        } yield assertTrue(
          rejected(triggered),
          rejected(scheduled),
          invoker.triggerCalls.isEmpty,
          invoker.scheduleCalls.isEmpty
        )
      }
    },
    test("wire trigger and schedule reject a method whose output is a stream before calling the host") {
      val invoker = new RecordingRpcInvoker
      val wire    = WireResolvedAgent(rpcWireType, RemoteAgentClient("ScopeWireAgent", invoker))
      val method  = wire.methodByName[Unit, AgentStream[String]]("produceStream")

      def rejected(error: Throwable): Boolean =
        error match {
          case error: IllegalArgumentException => error.getMessage.contains("live streams")
          case _                               => false
        }

      ZIO.fromFuture { implicit ec =>
        for {
          triggered <- wire.trigger(method, ()).failed
          scheduled <- wire.schedule(method, golem.Datetime.fromEpochMillis(1.0), ()).failed
        } yield assertTrue(
          method.outputContainsStream,
          !method.inputContainsStream,
          rejected(triggered),
          rejected(scheduled),
          invoker.triggerCalls.isEmpty,
          invoker.scheduleCalls.isEmpty
        )
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

  private def errorOf(future: Future[_]): Future[String] =
    future.transform(result => scala.util.Success(result.failed.map(_.getMessage).getOrElse("<succeeded>")))

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
    RemoteAgentClient(agentType.typeName, invoker)

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

  private val started: InvocationMetadata = {
    val agentId = golem.AgentId(java.util.UUID.fromString("0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d"))
    InvocationMetadata(
      agentId,
      golem.InvocationId(agentId, java.util.UUID.fromString("3f2a1b0c-9d8e-4f7a-b6c5-d4e3f2a1b0c9"))
    )
  }

  private final class RecordingRpcInvoker extends RpcInvoker {
    val invokeCalls    = mutable.ListBuffer.empty[(String, JsSchemaValueTree)]
    val scopeCardsSeen = mutable.ListBuffer.empty[Option[ScopeCard]]
    val triggerCalls   = mutable.ListBuffer.empty[(String, JsSchemaValueTree)]
    val scheduleCalls  = mutable.ListBuffer.empty[(golem.Datetime, String, JsSchemaValueTree)]

    private val invokeResults = mutable.Queue.empty[Either[AgentRpcError, Option[JsSchemaValueTree]]]

    var handleResult: Option[JsSchemaValueTree] = None

    def enqueueInvokeResult(result: Either[AgentRpcError, Option[JsSchemaValueTree]]): Unit =
      invokeResults.enqueue(result)

    override def invokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree,
      scopeCard: Option[ScopeCard]
    ): Future[InvocationResult[Option[JsSchemaValueTree]]] = {
      invokeCalls += ((functionName, input))
      scopeCardsSeen += scopeCard
      (if (invokeResults.nonEmpty) invokeResults.dequeue() else Right(None)) match {
        case Right(value) => Future.successful(InvocationResult(started, value))
        case Left(error)  => Future.failed(AgentRpcException(error))
      }
    }

    private def handle(): Invocation[Option[JsSchemaValueTree]] =
      Invocation(
        started.invocationId,
        new Invocation.Backend {
          def status(): Future[InvocationStatus]            = Future.successful(InvocationStatus.Pending)
          def awaitRaw(): Future[Option[JsSchemaValueTree]] = Future.successful(handleResult)
          def cancel(): Future[Boolean]                     = Future.successful(true)
        }
      )

    override def trigger(
      functionName: String,
      input: JsSchemaValueTree
    ): Future[Invocation[Option[JsSchemaValueTree]]] = {
      triggerCalls += ((functionName, input))
      Future.successful(handle())
    }

    override def schedule(
      datetime: golem.Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Future[Invocation[Option[JsSchemaValueTree]]] = {
      scheduleCalls += ((datetime, functionName, input))
      Future.successful(handle())
    }
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

    def produceStream(): Future[AgentStream[String]]
  }

  @agentDefinition(mode = DurabilityMode.Durable)
  trait ScopeWireAgent extends BaseAgent {
    class Id(val token: String)

    def rpcCall(input: SampleInput): Future[SampleOutput]

    def fireAndForget(event: String): Unit

    def sendCard(card: PermissionCard): Future[Boolean]

    def sendTemplate(card: PolymorphicPermissionCard): Future[PolymorphicPermissionCard]

    def produceStream(): Future[AgentStream[String]]
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
