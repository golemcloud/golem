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

package golem.runtime.macros

import golem.runtime.annotations.{
  agentDefinition,
  description,
  durableStreams,
  durableStreamSlot,
  endpoint,
  prompt,
  DurabilityMode
}
import golem.runtime.http.DurableStreamSlotSource
import golem.runtime.{AsyncImplementationMethod, MethodInvocation, OutputMetadata}
import golem.schema.{AgentStream, SchemaGraph, SchemaTypeBody}
import zio.blocks.schema.Schema
import zio.test._

import scala.concurrent.Future

private[macros] object AgentMetadataMacroTypes {

  sealed trait EchoResult
  object EchoResult {
    final case class Ok(value: Int)       extends EchoResult
    final case class Err(message: String) extends EchoResult

    implicit val schema: Schema[EchoResult] = Schema.derived
  }
}

/**
 * Verifies that the agent macros produce correct schema-native
 * [[golem.runtime.AgentMetadata]] / [[golem.runtime.AgentType]] /
 * [[golem.runtime.AgentImplementationType]] on the `golem:agent@2.0.0` model:
 * method names, parameter ordering + per-parameter schema bodies, durability
 * mode capture, and method invocation kinds.
 */
object AgentMetadataMacroSpec extends ZIOSpecDefault {
  import AgentMetadataMacroTypes._

  @agentDefinition()
  @description("Rust-style Echo agent for metadata parity")
  trait EchoAgent {
    class Id()

    @prompt("Echo the provided message")
    def echo(message: String): Future[String]

    def combine(left: String, right: Int): Future[String]
    def echoOption(value: Option[String]): Future[Option[String]]
    def echoResult(value: EchoResult): Future[EchoResult]
    def echoEither(value: Either[String, Int]): Future[Either[String, Int]]
  }

  @agentDefinition(mode = DurabilityMode.Ephemeral)
  trait EphemeralAgent {
    class Id()
    def ping(): Future[String]
  }

  @agentDefinition()
  trait DurableDefaultAgent { class Id(); def ping(): Future[String] }

  @agentDefinition(mode = DurabilityMode.Durable)
  trait DurableExplicitAgent { class Id(); def ping(): Future[String] }

  @agentDefinition()
  trait RpcParityAgent {
    class Id()
    def rpcCall(payload: String): Future[String]
    def rpcCallTrigger(payload: String): Unit
  }

  final case class ConcreteOnly(value: String)
  final case class StreamEnvelope(values: Option[List[AgentStream[String]]])

  @agentDefinition()
  trait ConcreteOnlyClientAgent {
    class Id(owner: ConcreteOnly, streams: StreamEnvelope)
    def roundTrip(value: ConcreteOnly): Future[ConcreteOnly]
    def consume(value: StreamEnvelope): Unit
  }

  @agentDefinition(mount = "/streams")
  trait DurableStreamAgent {
    class Id()

    @endpoint(method = "POST", path = "/plain")
    def plain(value: String): Future[String]

    @endpoint(method = "POST", path = "/process")
    @durableStreamSlot(
      endpointMethod = "POST",
      endpointPath = "/process",
      source = "input",
      slot = "request",
      name = "body",
      contentType = "application/json"
    )
    @durableStreamSlot(
      endpointMethod = "POST",
      endpointPath = "/process",
      source = "output",
      slot = "response",
      name = "result",
      contentType = "text/plain"
    )
    @durableStreams(
      endpointMethod = "POST",
      endpointPath = "/process",
      allowExternalWrites = true,
      allowStreamDelete = false,
      allowInvocationDelete = false,
      maxConcurrentReadersPerStream = 8,
      maxAppendRequestsPerSecondPerStream = 25
    )
    def process(value: String): Future[String]

    @endpoint(method = "POST", path = "/defaults")
    @durableStreamSlot(source = "input", slot = "value")
    @durableStreams()
    def defaults(value: String): Future[String]
  }

  private final class EphemeralAgentImpl extends EphemeralAgent {
    override def ping(): Future[String] = Future.successful("pong")
  }

  private final class DurableDefaultAgentImpl extends DurableDefaultAgent {
    override def ping(): Future[String] = Future.successful("durable-default")
  }

  private final class DurableExplicitAgentImpl extends DurableExplicitAgent {
    override def ping(): Future[String] = Future.successful("durable-explicit")
  }

  private val echoMetadata            = AgentMacros.agentMetadata[EchoAgent]
  private val ephemeralMetadata       = AgentMacros.agentMetadata[EphemeralAgent]
  private val durableDefaultMetadata  = AgentMacros.agentMetadata[DurableDefaultAgent]
  private val durableExplicitMetadata = AgentMacros.agentMetadata[DurableExplicitAgent]
  private val durableDefaultImplType  =
    AgentImplementationMacro.implementationType[DurableDefaultAgent](new DurableDefaultAgentImpl)
  private val durableExplicitImplType =
    AgentImplementationMacro.implementationType[DurableExplicitAgent](new DurableExplicitAgentImpl)
  private val rpcImplType = AgentImplementationMacro.implementationType[RpcParityAgent](new RpcParityAgent {
    override def rpcCall(payload: String): Future[String] = Future.successful(payload)
    override def rpcCallTrigger(payload: String): Unit    = ()
  })
  private val durableStreamMetadata = AgentMacros.agentMetadata[DurableStreamAgent]

  /**
   * The effective root body of a graph, dereferencing a top-level named ref.
   */
  private def rootBody(g: SchemaGraph): SchemaTypeBody = g.root.body match {
    case SchemaTypeBody.RefType(id) => g.defs(id).body.body
    case other                      => other
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("AgentMetadataMacroSpec")(
      test("compiler-emitted wire metadata equals dynamic metadata encoding") {
        val compiled = AgentDefinitionMacro.generateWire[EchoAgent]
        val dynamic  = golem.runtime.WireAgentMetadata.fromModel(echoMetadata)
        assertTrue(
          compiled == dynamic,
          AgentDefinitionMacro
            .generateWire[EphemeralAgent] == golem.runtime.WireAgentMetadata.fromModel(ephemeralMetadata)
        )
      },
      test("EchoAgent metadata exposes all method names") {
        val names = echoMetadata.methods.map(_.name).sorted
        assertTrue(
          names == List("combine", "echo", "echoEither", "echoOption", "echoResult"),
          echoMetadata.description.contains("Rust-style Echo agent for metadata parity")
        )
      },
      test("EchoAgent combine method keeps parameter ordering and per-parameter schema") {
        val method = echoMetadata.methods.find(_.name == "combine").get
        val params = method.input.userSupplied
        assertTrue(
          params.map(_.name) == List("left", "right"),
          rootBody(params.head.graph) == SchemaTypeBody.StringType,
          rootBody(params(1).graph) == SchemaTypeBody.S32Type()
        )
      },
      test("EchoAgent Either method derives input and output schemas implicitly") {
        val method   = echoMetadata.methods.find(_.name == "echoEither").get
        val expected = SchemaTypeBody.ResultType(
          Some(golem.schema.t.s32),
          Some(golem.schema.t.string)
        )
        assertTrue(
          rootBody(method.input.userSupplied.head.graph) == expected,
          rootBody(method.output.asInstanceOf[OutputMetadata.Single].graph) == expected
        )
      },
      test("Agent metadata captures trait-level mode annotation") {
        assertTrue(ephemeralMetadata.mode.contains("ephemeral"))
      },
      test("Agent metadata omits mode when durable annotation is not provided") {
        assertTrue(durableDefaultMetadata.mode.isEmpty)
      },
      test("Agent metadata omits durable default (even when explicitly set via agentDefinition)") {
        assertTrue(durableExplicitMetadata.mode.forall(_ == "durable"))
      },
      test("AgentImplementationMacro preserves annotated agent mode") {
        val implType = AgentImplementationMacro.implementationType[EphemeralAgent](new EphemeralAgentImpl)
        assertTrue(implType.metadata.mode.contains("ephemeral"))
      },
      test("AgentImplementationMacro leaves mode unset for durable defaults") {
        assertTrue(durableDefaultImplType.metadata.mode.forall(_ == "durable"))
      },
      test("AgentImplementationMacro preserves durable annotations in implementation metadata") {
        assertTrue(durableExplicitImplType.metadata.mode.forall(_ == "durable"))
      },
      test("AgentClientMacro produces fire-and-forget invocation for Unit-returning method") {
        val agentType     = AgentClientMacro.agentType[RpcParityAgent]
        val triggerMethod =
          agentType.methods.find(_.metadata.name == "rpcCallTrigger").get
        assertTrue(triggerMethod.invocation == MethodInvocation.FireAndForget)
      },
      test("wire client derivation does not require owned schema codecs") {
        val client = AgentClientMacro.wireType[ConcreteOnlyClientAgent]
        assertTrue(
          client.metadata.name == "ConcreteOnlyClientAgent",
          client.ctorContainsStream,
          client.methods.find(_.name == "roundTrip").exists(!_.inputContainsStream),
          client.methods.find(_.name == "consume").exists(_.inputContainsStream)
        )
      },
      test("AgentImplementationMacro preserves method invocation kinds") {
        val awaitable =
          rpcImplType.methods.collectFirst {
            case m: AsyncImplementationMethod[RpcParityAgent @unchecked, _, _] if m.metadata.name == "rpcCall" =>
              m
          }
        assertTrue(awaitable.isDefined)
      },
      test("durable stream annotations are grouped into their selected endpoint") {
        val process  = durableStreamMetadata.methods.find(_.name == "process").get.httpEndpoints.head.durableStreams.get
        val defaults =
          durableStreamMetadata.methods.find(_.name == "defaults").get.httpEndpoints.head.durableStreams.get
        val plain = durableStreamMetadata.methods.find(_.name == "plain").get.httpEndpoints.head
        assertTrue(
          process.slots.map(_.source) == List(
            DurableStreamSlotSource.Input("request"),
            DurableStreamSlotSource.Output("response")
          ),
          process.slots.map(_.name) == List(Some("body"), Some("result")),
          process.slots.map(_.contentType) == List(Some("application/json"), Some("text/plain")),
          process.allowExternalWrites.contains(true),
          process.allowStreamDelete.contains(false),
          process.allowInvocationDelete.contains(false),
          process.load.flatMap(_.maxConcurrentReadersPerStream).contains(8),
          process.load.flatMap(_.maxAppendRequestsPerSecondPerStream).contains(25),
          defaults.allowExternalWrites.isEmpty,
          defaults.allowStreamDelete.isEmpty,
          defaults.allowInvocationDelete.isEmpty,
          defaults.load.isEmpty,
          plain.durableStreams.isEmpty
        )
      }
    )
}
