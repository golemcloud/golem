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

package golem.runtime

import golem.BaseAgent
import golem.runtime.annotations.{DurabilityMode, agentDefinition, agentImplementation, description, prompt}
import golem.runtime.autowire.{AgentDefinition, AgentImplementation, AgentMode}
import zio.test._
import zio.blocks.schema.Schema

import scala.concurrent.Future

object AgentRegistrationMetadataSpec extends ZIOSpecDefault {

  @agentDefinition("meta-agent")
  @description("An agent used for metadata tests.")
  trait MetaAgent extends BaseAgent {
    class Id()
    @description("Echoes input.")
    @prompt("Say hello.")
    def echo(s: String): Future[String]

    @description("Adds two ints.")
    def add(a: Int, b: Int): Future[Int]

    def noAnnotation(): Future[String]
  }

  @agentImplementation()
  final class MetaAgentImpl() extends MetaAgent {
    override def echo(s: String): Future[String]  = Future.successful(s)
    override def add(a: Int, b: Int): Future[Int] = Future.successful(a + b)
    override def noAnnotation(): Future[String]   = Future.successful("ok")
  }

  private lazy val defn: AgentDefinition[MetaAgent] =
    AgentImplementation.registerClass[MetaAgent, MetaAgentImpl]

  // ---------------------------------------------------------------------------
  // Ephemeral mode
  // ---------------------------------------------------------------------------

  @agentDefinition("ephemeral-meta-agent", mode = DurabilityMode.Ephemeral)
  trait EphemeralMetaAgent extends BaseAgent {
    class Id()
    def ping(): Future[String]
  }

  @agentImplementation()
  final class EphemeralMetaAgentImpl() extends EphemeralMetaAgent {
    override def ping(): Future[String] = Future.successful("pong")
  }

  private lazy val ephDefn: AgentDefinition[EphemeralMetaAgent] =
    AgentImplementation.registerClass[EphemeralMetaAgent, EphemeralMetaAgentImpl]

  // ---------------------------------------------------------------------------
  // Constructor type agent
  // ---------------------------------------------------------------------------

  final case class MetaConfig(host: String, port: Int)
  object MetaConfig { implicit val schema: Schema[MetaConfig] = Schema.derived }

  @agentDefinition("ctor-meta-agent")
  @description("Agent with case class constructor.")
  trait CtorMetaAgent extends BaseAgent {
    class Id(val host: String, val port: Int)
    def info(): Future[String]
  }

  @agentImplementation()
  final class CtorMetaAgentImpl(private val host: String, private val port: Int) extends CtorMetaAgent {
    override def info(): Future[String] = Future.successful(s"$host:$port")
  }

  private lazy val ctorDefn: AgentDefinition[CtorMetaAgent] =
    AgentImplementation.registerClass[CtorMetaAgent, CtorMetaAgentImpl]

  // ---------------------------------------------------------------------------
  // Explicit Durable mode
  // ---------------------------------------------------------------------------

  @agentDefinition("explicit-durable-agent", mode = DurabilityMode.Durable)
  trait ExplicitDurableAgent extends BaseAgent {
    class Id()
    def ping(): Future[String]
  }

  @agentImplementation()
  final class ExplicitDurableAgentImpl() extends ExplicitDurableAgent {
    override def ping(): Future[String] = Future.successful("pong")
  }

  private lazy val durDefn: AgentDefinition[ExplicitDurableAgent] =
    AgentImplementation.registerClass[ExplicitDurableAgent, ExplicitDurableAgentImpl]

  def spec = suite("AgentRegistrationMetadataSpec")(
    test("registered agent has correct typeName") {
      assertTrue(defn.typeName == "meta-agent")
    },
    test("metadata contains all methods") {
      val names = defn.methodMetadata.map(_.metadata.name).toSet
      assertTrue(names == Set("echo", "add", "noAnnotation"))
    },
    test("method count matches trait method count") {
      assertTrue(defn.methodMetadata.size == 3)
    },
    test("echo method has description from @description") {
      val echo = defn.methodMetadata.find(_.metadata.name == "echo").get
      assertTrue(echo.metadata.description.contains("Echoes input."))
    },
    test("echo method has prompt from @prompt") {
      val echo = defn.methodMetadata.find(_.metadata.name == "echo").get
      assertTrue(echo.metadata.prompt.contains("Say hello."))
    },
    test("add method has description but no prompt") {
      val add = defn.methodMetadata.find(_.metadata.name == "add").get
      assertTrue(
        add.metadata.description.contains("Adds two ints."),
        add.metadata.prompt.isEmpty
      )
    },
    test("unannotated method has no description and no prompt") {
      val m = defn.methodMetadata.find(_.metadata.name == "noAnnotation").get
      assertTrue(
        m.metadata.description.isEmpty,
        m.metadata.prompt.isEmpty
      )
    },
    test("default mode is Durable") {
      assertTrue(defn.mode == AgentMode.Durable)
    },
    test("ephemeral agent has Ephemeral mode") {
      assertTrue(ephDefn.mode == AgentMode.Ephemeral)
    },
    test("constructor agent registration succeeds") {
      assertTrue(ctorDefn.typeName == "ctor-meta-agent")
    },
    test("constructor agent has correct method count") {
      assertTrue(
        ctorDefn.methodMetadata.size == 1,
        ctorDefn.methodMetadata.head.metadata.name == "info"
      )
    },
    test("explicit durable agent has Durable mode") {
      assertTrue(durDefn.mode == AgentMode.Durable)
    },
    test("echo method inputSchema has tuple tag") {
      val echo  = defn.methodMetadata.find(_.metadata.name == "echo").get
      val input = echo.inputSchema
      assertTrue(input.tag == "tuple")
    },
    test("echo method outputSchema has tuple tag") {
      val echo   = defn.methodMetadata.find(_.metadata.name == "echo").get
      val output = echo.outputSchema
      assertTrue(output.tag == "tuple")
    },
    test("add method inputSchema has tuple tag with elements") {
      val add   = defn.methodMetadata.find(_.metadata.name == "add").get
      val input = add.inputSchema
      assertTrue(input.tag == "tuple")
    },
    test("multiple methods can have different descriptions") {
      val echo = defn.methodMetadata.find(_.metadata.name == "echo").get
      val add  = defn.methodMetadata.find(_.metadata.name == "add").get
      assertTrue(echo.metadata.description != add.metadata.description)
    },
    test("agent trait description is captured in metadata") {
      assertTrue(defn.metadata.description.contains("An agent used for metadata tests."))
    }
  )
}
