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

package golem.runtime

import golem.BaseAgent
import golem.host.SchemaWireInterop
import golem.host.js.schema.{JsAgentType, JsNamedField, JsSchemaGraph}
import golem.runtime.annotations.{DurabilityMode, agentDefinition, agentImplementation, description}
import golem.runtime.autowire.{AgentDefinition, AgentImplementation}
import golem.schema.{IntoSchema, SchemaGraph, SchemaType}
import golem.schema.wire.SchemaWire
import zio.blocks.schema.Schema
import zio.test._

import scala.concurrent.Future
import scala.scalajs.js

/**
 * Slice 4 — exercises the live declaration-assembly path end-to-end:
 * `AgentMetadata` (compile-time, from the macros) ->
 * [[golem.runtime.autowire.AgentRequestBuilder]] ->
 * [[golem.runtime.autowire.AgentTypeEncoderV2]] -> the host-facing
 * [[JsAgentType]] (`golem:agent@2.0.0`).
 *
 * Where [[golem.runtime.autowire.AgentTypeEncoderV2Spec]] feeds the encoder a
 * hand-built `AgentRequest` and `AgentRegistrationMetadataSpec` stops at the
 * `AgentMetadata`, this spec starts from a real `@agentImplementation` and
 * asserts on the merged `agent-type` actually emitted by
 * `AgentDefinition.agentType`. It pins the declaration wiring: mode/description
 * propagation, constructor + method `parameters` (names, `user-supplied`
 * source, resolvable per-field schema indices into the single merged graph) and
 * `output-schema = unit|single`.
 */
object AgentTypeDeclarationSpec extends ZIOSpecDefault {

  final case class Reading(value: Int, label: String)
  object Reading {
    implicit val schema: Schema[Reading] = Schema.derived
  }

  @agentDefinition("declaration-agent", mode = DurabilityMode.Ephemeral)
  @description("Declaration path agent.")
  trait DeclAgent extends BaseAgent {
    class Id(val host: String, val port: Int)

    @description("Echoes a string.")
    def echo(s: String): Future[String]

    @description("Builds a reading from two values.")
    def build(value: Int, label: String): Future[Reading]

    def ping(): Future[Unit]
  }

  @agentImplementation()
  final class DeclAgentImpl(private val host: String, private val port: Int) extends DeclAgent {
    override def echo(s: String): Future[String]                   = Future.successful(s)
    override def build(value: Int, label: String): Future[Reading] =
      Future.successful(Reading(value, label))
    override def ping(): Future[Unit] = Future.unit
  }

  private lazy val defn: AgentDefinition[DeclAgent] =
    AgentImplementation.registerClass[DeclAgent, DeclAgentImpl]

  private lazy val agentType: JsAgentType = defn.agentType

  // --- JS reflection helpers (mirror AgentTypeEncoderV2Spec conventions) -----

  private def dyn(o: js.Any): js.Dynamic = o.asInstanceOf[js.Dynamic]
  private def rawVal(o: js.Any): js.Any  = dyn(o).selectDynamic("val")
  private def tag(o: js.Any): String     = dyn(o).selectDynamic("tag").asInstanceOf[String]

  private def graphOf[A](implicit ev: IntoSchema[A]): SchemaGraph = ev.graph

  /**
   * Resolve the `SchemaType` reachable from a `type-node-index` in a JS graph.
   */
  private def typeAt(jsGraph: JsSchemaGraph, index: Int): SchemaType = {
    val wit = SchemaWireInterop.graphFromJs(jsGraph)
    SchemaWire.schemaGraphFromWit(wit.copy(root = index)).root
  }

  private def inputFields(inputSchema: js.Any): js.Array[JsNamedField] =
    rawVal(inputSchema).asInstanceOf[js.Array[JsNamedField]]

  private def method(name: String): golem.host.js.schema.JsAgentMethod =
    agentType.methods.find(_.name == name).getOrElse(sys.error(s"method not found: $name"))

  override def spec: Spec[TestEnvironment, Any] =
    suite("AgentTypeDeclarationSpec")(
      test("top-level declaration carries typeName/description/mode/sourceLanguage") {
        assertTrue(
          agentType.typeName == "declaration-agent",
          agentType.description == "Declaration path agent.",
          agentType.mode == "ephemeral",
          agentType.sourceLanguage == "scala",
          agentType.methods.length == 3
        )
      },
      test("constructor parameters come from the Id class, all user-supplied, resolvable") {
        val fields = inputFields(agentType.constructor.inputSchema)
        assertTrue(
          tag(agentType.constructor.inputSchema) == "parameters",
          fields.length == 2,
          fields(0).name == "host",
          fields(1).name == "port",
          tag(fields(0).source) == "user-supplied",
          tag(fields(1).source) == "user-supplied",
          typeAt(agentType.schema, fields(0).schema) == graphOf[String].root,
          typeAt(agentType.schema, fields(1).schema) == graphOf[Int].root
        )
      },
      test("single-parameter method input maps the parameter by name + resolvable index") {
        val echo   = method("echo")
        val fields = inputFields(echo.inputSchema)
        assertTrue(
          echo.description == "Echoes a string.",
          fields.length == 1,
          fields(0).name == "s",
          tag(fields(0).source) == "user-supplied",
          typeAt(agentType.schema, fields(0).schema) == graphOf[String].root
        )
      },
      test("multi-parameter method preserves parameter order and per-field schemas") {
        val build  = method("build")
        val fields = inputFields(build.inputSchema)
        assertTrue(
          fields.length == 2,
          fields(0).name == "value",
          fields(1).name == "label",
          typeAt(agentType.schema, fields(0).schema) == graphOf[Int].root,
          typeAt(agentType.schema, fields(1).schema) == graphOf[String].root
        )
      },
      test("method returning a value emits output-schema single(index) resolving to the type") {
        val build = method("build")
        val out   = build.outputSchema
        assertTrue(
          tag(out) == "single",
          typeAt(agentType.schema, rawVal(out).asInstanceOf[Int]) == graphOf[Reading].root
        )
      },
      test("method returning Unit emits output-schema unit with no input fields") {
        val ping = method("ping")
        assertTrue(
          tag(ping.outputSchema) == "unit",
          inputFields(ping.inputSchema).length == 0
        )
      }
    )
}
