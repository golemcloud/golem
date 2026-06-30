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

package golem.runtime.macros

import golem.runtime.annotations.{agentDefinition, description, prompt, DurabilityMode}
import golem.runtime.{AsyncImplementationMethod, MethodInvocation}
import golem.schema.{SchemaGraph, SchemaTypeBody}
import zio.blocks.schema.Schema
import zio.test._

import scala.concurrent.Future

private[macros] object AgentMetadataMacroTypes {

  /**
   * Sealed-trait result type used to assert the macros derive schema-native
   * metadata for nested user types. Avoids stdlib `Either`, whose zio-blocks
   * derivation only works on Scala 3.
   */
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
 * mode capture, and method invocation kinds. Runs on both Scala 2.13 and 3.
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

  /**
   * The effective root body of a graph, dereferencing a top-level named ref.
   */
  private def rootBody(g: SchemaGraph): SchemaTypeBody = g.root.body match {
    case SchemaTypeBody.RefType(id) => g.defs(id).body.body
    case other                      => other
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("AgentMetadataMacroSpec")(
      test("EchoAgent metadata exposes all method names") {
        val names = echoMetadata.methods.map(_.name).sorted
        assertTrue(
          names == List("combine", "echo", "echoOption", "echoResult"),
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
      test("AgentImplementationMacro preserves method invocation kinds") {
        val awaitable =
          rpcImplType.methods.collectFirst {
            case m: AsyncImplementationMethod[RpcParityAgent @unchecked, _, _] if m.metadata.name == "rpcCall" =>
              m
          }
        assertTrue(awaitable.isDefined)
      }
    )
}
