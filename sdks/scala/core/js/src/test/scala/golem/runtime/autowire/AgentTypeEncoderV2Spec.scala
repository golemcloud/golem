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

package golem.runtime.autowire

import golem.host.SchemaWireInterop
import golem.host.js.JsSnapshotting
import golem.host.js.schema._
import golem.schema.{IntoSchema, SchemaConflictError, SchemaGraph, SchemaType, SchemaTypeBody, SchemaTypeDef}
import golem.schema.wire.SchemaWire
import zio.blocks.schema.Schema
import zio.test._

import scala.collection.immutable.ListMap
import scala.scalajs.js

/**
 * Slice 4a — [[AgentTypeEncoderV2]] builds ONE merged `schema-graph` per agent
 * and emits every constructor / method / config schema root as a
 * `type-node-index` into that single graph (§4.22). These tests pin the
 * load-bearing invariants: deterministic graph merge by stable type-id,
 * conflict rejection, `output-schema = unit|single`, `field-source`
 * user-supplied vs auto-injected(principal), `agent-config-declaration`
 * indices, and the exact v2 JS shapes from `golem_agent_2_0_0_common.d.ts`.
 */
object AgentTypeEncoderV2Spec extends ZIOSpecDefault {

  final case class Point(x: Int, y: Int)
  object Point {
    implicit val schema: Schema[Point] = Schema.derived
  }

  final case class Greeting(text: String)
  object Greeting {
    implicit val schema: Schema[Greeting] = Schema.derived
  }

  private def graphOf[A](implicit ev: IntoSchema[A]): SchemaGraph = ev.graph

  private val snapshotting: JsSnapshotting = JsSnapshotting.disabled

  // --- JS reflection helpers (mirror SchemaWireInteropSpec conventions) ------

  private def dyn(o: js.Any): js.Dynamic = o.asInstanceOf[js.Dynamic]
  private def rawVal(o: js.Any): js.Any  = dyn(o).selectDynamic("val")
  private def tag(o: js.Any): String     = dyn(o).selectDynamic("tag").asInstanceOf[String]

  /**
   * Resolve the `SchemaType` reachable from a `type-node-index` in a JS graph.
   */
  private def typeAt(jsGraph: JsSchemaGraph, index: Int): SchemaType = {
    val wit = SchemaWireInterop.graphFromJs(jsGraph)
    SchemaWire.schemaGraphFromWit(wit.copy(root = index)).root
  }

  /** A minimal request: no-arg constructor, a single unit method. */
  private def baseRequest(
    methods: List[AgentTypeEncoderV2.Method] = Nil,
    constructorParams: List[AgentTypeEncoderV2.Param] = Nil,
    config: List[AgentTypeEncoderV2.ConfigDecl] = Nil
  ): AgentTypeEncoderV2.AgentRequest =
    AgentTypeEncoderV2.AgentRequest(
      typeName = "MyAgent",
      description = "an agent",
      mode = "durable",
      constructor = AgentTypeEncoderV2.Constructor(description = "ctor", params = constructorParams),
      methods = methods,
      snapshotting = snapshotting,
      config = config
    )

  override def spec: Spec[TestEnvironment, Any] =
    suite("AgentTypeEncoderV2Spec")(
      test("top-level scalar JS shape: typeName/description/mode/sourceLanguage") {
        val at = AgentTypeEncoderV2.encode(baseRequest())
        assertTrue(
          at.typeName == "MyAgent",
          at.description == "an agent",
          at.mode == "durable",
          at.sourceLanguage == "scala",
          at.methods.length == 0,
          at.dependencies.length == 0,
          at.config.length == 0
        )
      },
      test("constructor input-schema uses parameters + per-field source/index") {
        val req = baseRequest(constructorParams =
          List(
            AgentTypeEncoderV2.Param("p", AgentTypeEncoderV2.FieldSource.UserSupplied, graphOf[Point]),
            AgentTypeEncoderV2
              .Param("g", AgentTypeEncoderV2.FieldSource.AutoInjectedPrincipal, graphOf[Greeting])
          )
        )
        val at     = AgentTypeEncoderV2.encode(req)
        val input  = at.constructor.inputSchema
        val fields = rawVal(input).asInstanceOf[js.Array[JsNamedField]]
        assertTrue(
          tag(input) == "parameters",
          fields.length == 2,
          fields(0).name == "p",
          tag(fields(0).source) == "user-supplied",
          fields(1).name == "g",
          tag(fields(1).source) == "auto-injected",
          rawVal(fields(1).source).asInstanceOf[String] == "principal"
        ) &&
        assertTrue(
          // each field's `schema` index resolves to its own type in the merged graph
          typeAt(at.schema, fields(0).schema) == graphOf[Point].root,
          typeAt(at.schema, fields(1).schema) == graphOf[Greeting].root
        )
      },
      test("method output-schema: unit -> { tag: unit }") {
        val req = baseRequest(methods =
          List(
            AgentTypeEncoderV2.Method(
              name = "fireAndForget",
              description = "no result",
              params = Nil,
              output = None
            )
          )
        )
        val at = AgentTypeEncoderV2.encode(req)
        assertTrue(tag(at.methods(0).outputSchema) == "unit")
      },
      test("method output-schema: single(index) resolves to the output type") {
        val req = baseRequest(methods =
          List(
            AgentTypeEncoderV2.Method(
              name = "compute",
              description = "returns a point",
              params = Nil,
              output = Some(graphOf[Point])
            )
          )
        )
        val at  = AgentTypeEncoderV2.encode(req)
        val out = at.methods(0).outputSchema
        assertTrue(tag(out) == "single") &&
        assertTrue(typeAt(at.schema, rawVal(out).asInstanceOf[Int]) == graphOf[Point].root)
      },
      test("method input params carry name + source + resolvable index") {
        val req = baseRequest(methods =
          List(
            AgentTypeEncoderV2.Method(
              name = "m",
              description = "d",
              params = List(
                AgentTypeEncoderV2.Param("a", AgentTypeEncoderV2.FieldSource.UserSupplied, graphOf[Point]),
                AgentTypeEncoderV2.Param("b", AgentTypeEncoderV2.FieldSource.UserSupplied, graphOf[Greeting])
              ),
              output = None
            )
          )
        )
        val at     = AgentTypeEncoderV2.encode(req)
        val fields = rawVal(at.methods(0).inputSchema).asInstanceOf[js.Array[JsNamedField]]
        assertTrue(
          fields.length == 2,
          fields(0).name == "a",
          fields(1).name == "b",
          typeAt(at.schema, fields(0).schema) == graphOf[Point].root,
          typeAt(at.schema, fields(1).schema) == graphOf[Greeting].root
        )
      },
      test("config declarations carry source/path + resolvable valueType index") {
        val req = baseRequest(config =
          List(
            AgentTypeEncoderV2.ConfigDecl("local", List("db", "host"), graphOf[Greeting])
          )
        )
        val at  = AgentTypeEncoderV2.encode(req)
        val cfg = at.config(0)
        assertTrue(
          cfg.source == "local",
          cfg.path.toList == List("db", "host"),
          typeAt(at.schema, cfg.valueType) == graphOf[Greeting].root
        )
      },
      test("shared defs are merged once: same type used twice has one def entry") {
        // Two constructor params of the same record type must merge to a single
        // def (no duplicate, no conflict), and both roots must resolve equally.
        val req = baseRequest(constructorParams =
          List(
            AgentTypeEncoderV2.Param("p1", AgentTypeEncoderV2.FieldSource.UserSupplied, graphOf[Point]),
            AgentTypeEncoderV2.Param("p2", AgentTypeEncoderV2.FieldSource.UserSupplied, graphOf[Point])
          )
        )
        val at        = AgentTypeEncoderV2.encode(req)
        val wit       = SchemaWireInterop.graphFromJs(at.schema)
        val pointDefs = wit.defs.count(d => d.name.contains("Point"))
        val fields    = rawVal(at.constructor.inputSchema).asInstanceOf[js.Array[JsNamedField]]
        assertTrue(
          pointDefs == 1,
          typeAt(at.schema, fields(0).schema) == graphOf[Point].root,
          typeAt(at.schema, fields(1).schema) == graphOf[Point].root
        )
      },
      test("conflicting same-id defs are rejected with SchemaConflictError") {
        val id = "golem.test.Conflicting"
        val g1 = SchemaGraph(
          ListMap(id -> SchemaTypeDef(SchemaType(SchemaTypeBody.RecordType(Nil)), Some("Conflicting"))),
          SchemaType(SchemaTypeBody.RefType(id))
        )
        val g2 = SchemaGraph(
          ListMap(id -> SchemaTypeDef(SchemaType(SchemaTypeBody.StringType), Some("Conflicting"))),
          SchemaType(SchemaTypeBody.RefType(id))
        )
        val req = baseRequest(constructorParams =
          List(
            AgentTypeEncoderV2.Param("a", AgentTypeEncoderV2.FieldSource.UserSupplied, g1),
            AgentTypeEncoderV2.Param("b", AgentTypeEncoderV2.FieldSource.UserSupplied, g2)
          )
        )
        val result = scala.util.Try(AgentTypeEncoderV2.encode(req))
        assertTrue(result.failed.toOption.exists(_.isInstanceOf[SchemaConflictError]))
      },
      test("schema.root is a structural placeholder (empty record)") {
        // §4.22: schema.root is not the semantic root; the encoder finishes with
        // a placeholder empty-record root.
        val at  = AgentTypeEncoderV2.encode(baseRequest())
        val wit = SchemaWireInterop.graphFromJs(at.schema)
        assertTrue(wit.typeNodes(wit.root).body == golem.schema.wire.WitSchemaTypeBody.RecordType(Vector.empty))
      }
    )
}
