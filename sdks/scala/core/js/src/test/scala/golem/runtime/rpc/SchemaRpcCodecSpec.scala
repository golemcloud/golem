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

import golem.host.SchemaWireInterop
import golem.host.js.schema.JsSchemaValueTree
import golem.runtime.autowire.SchemaPayload
import golem.schema._
import golem.schema.wire.SchemaWire
import zio.blocks.schema.Schema
import zio.test._

/**
 * Slice 4b — [[SchemaRpcCodec]] is the v2 (`golem:agent/host@2.0.0`)
 * RPC-boundary codec: it encodes the parameter-list `schema-value-tree`,
 * decodes the optional result tree (`unit` => `none`, `single` => `some`), and
 * carries `typed-schema-value`s for config / custom errors. These tests pin the
 * round-trips and the option/unit policy against the TS client semantics.
 */
object SchemaRpcCodecSpec extends ZIOSpecDefault {

  // Parameter-list records (the macro shapes method/constructor `In` this way).
  final case class Args2(a: Int, b: String)
  object Args2 {
    implicit val schema: Schema[Args2] = Schema.derived
  }

  final case class NoArgs()
  object NoArgs {
    implicit val schema: Schema[NoArgs] = Schema.derived
  }

  final case class Result(value: Int, label: String)
  object Result {
    implicit val schema: Schema[Result] = Schema.derived
  }

  final case class Config(host: String, port: Int)
  object Config {
    implicit val schema: Schema[Config] = Schema.derived
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("SchemaRpcCodecSpec")(
      suite("arguments (parameter-list value tree)")(
        test("encodeArgs/decodeArgs round-trip for a multi-field param list") {
          val in   = Args2(7, "hello")
          val tree = SchemaRpcCodec.encodeArgs(in)
          assertTrue(SchemaRpcCodec.decodeArgs[Args2](tree) == Right(in))
        },
        test("encodeArgs/decodeArgs round-trip for an empty param list") {
          val in   = NoArgs()
          val tree = SchemaRpcCodec.encodeArgs(in)
          assertTrue(SchemaRpcCodec.decodeArgs[NoArgs](tree) == Right(in))
        },
        test("encodeArgs equals SchemaPayload.encode (single value-tree hub)") {
          val in       = Args2(1, "x")
          val viaCodec = SchemaWireInterop.valueTreeFromJs(SchemaRpcCodec.encodeArgs(in))
          val viaHub   = SchemaWireInterop.valueTreeFromJs(SchemaPayload.encode(in))
          assertTrue(viaCodec == viaHub)
        }
      ),
      suite("results (option<schema-value-tree>)")(
        test("encodeUnitResult is absent (none on the wire)") {
          assertTrue(SchemaRpcCodec.encodeUnitResult.isEmpty)
        },
        test("decodeUnitResult is always () regardless of presence") {
          val absent: Option[JsSchemaValueTree]  = None
          val present: Option[JsSchemaValueTree] = Some(SchemaPayload.encode(Result(1, "a")))
          assertTrue(
            SchemaRpcCodec.decodeUnitResult(absent) == Right(()),
            SchemaRpcCodec.decodeUnitResult(present) == Right(())
          )
        },
        test("encodeSingleResult/decodeSingleResult round-trip") {
          val out  = Result(42, "answer")
          val some = SchemaRpcCodec.encodeSingleResult(out)
          assertTrue(
            some.isDefined,
            SchemaRpcCodec.decodeSingleResult[Result](some) == Right(out)
          )
        },
        test("decodeSingleResult on absent result is an error") {
          val decoded = SchemaRpcCodec.decodeSingleResult[Result](None)
          assertTrue(decoded.isLeft)
        }
      ),
      suite("typed-schema-value (config values, custom errors)")(
        test("encodeTyped/decodeTyped round-trip") {
          val cfg   = Config("localhost", 5432)
          val typed = SchemaRpcCodec.encodeTyped(cfg)
          assertTrue(SchemaRpcCodec.decodeTyped[Config](typed) == Right(cfg))
        },
        test("encodeTyped carries the self-contained graph of A") {
          val cfg      = Config("h", 1)
          val typed    = SchemaRpcCodec.encodeTyped(cfg)
          val graph    = SchemaWireInterop.graphFromJs(typed.graph)
          val expected = SchemaWire.schemaGraphToWit(IntoSchema[Config].graph)
          assertTrue(graph == expected)
        },
        test("typedConfigValue carries path + typed-schema-value") {
          val entry = SchemaRpcCodec.typedConfigValue(List("db", "primary"), Config("h", 2))
          assertTrue(
            entry.path.toList == List("db", "primary"),
            SchemaRpcCodec.decodeTyped[Config](entry.value) == Right(Config("h", 2))
          )
        },
        test("typedSchemaValueFromSchemaGraphJson decodes serialized rich schema graphs") {
          val graphJson =
            """{"root":{"kind":"path","value":{"spec":{"direction":"input","kind":"file","allowedExtensions":["txt"]}}}}"""
          val typed = SchemaRpcCodec.typedSchemaValueFromSchemaGraphJson(graphJson, SchemaValue.PathValue("notes.txt"))
          assertTrue(
            typed.value == SchemaValue.PathValue("notes.txt"),
            typed.graph.root == SchemaType(
              SchemaTypeBody.PathType(PathSpec(PathDirection.Input, PathKind.File, None, Some(List("txt"))))
            )
          )
        },
        test("schemaGraphFromJson preserves stringified numeric-bound precision") {
          val graphJson =
            """{"root":{"kind":"u64","value":{"restrictions":{"min":{"kind":"unsigned","value":"18446744073709551615"},"unit":"bytes"}}}}"""
          val graph = SchemaRpcCodec.schemaGraphFromJson(graphJson)
          assertTrue(
            graph.root == SchemaType(
              SchemaTypeBody.U64Type(Some(NumericRestrictions(Some(NumericBound.Unsigned(-1L)), None, Some("bytes"))))
            )
          )
        },
        test("schemaGraphFromJson preserves unstructured roles") {
          val graphJson =
            """{"root":{"kind":"variant","value":{"cases":[{"name":"inline","payload":{"kind":"text","value":{"restrictions":{}}}},{"name":"url","payload":{"kind":"url","value":{"restrictions":{}}}}]},"metadata":{"role":{"tag":"unstructured-text"}}}}"""
          assertTrue(scala.util.Try(SchemaRpcCodec.schemaGraphFromJson(graphJson)).isSuccess)
        },
        test("schemaGraphFromJson preserves unstructured role tags when re-encoding config graphs") {
          val graphJson =
            """{"root":{"kind":"variant","value":{"cases":[{"name":"inline","payload":{"kind":"text","value":{"restrictions":{}}}},{"name":"url","payload":{"kind":"url","value":{"restrictions":{}}}}],"metadata":{"role":{"tag":"unstructured-text"}}}}}"""
          val graph   = SchemaRpcCodec.schemaGraphFromJson(graphJson)
          val jsGraph = SchemaWireInterop.graphToJs(SchemaWire.schemaGraphToWit(graph))
          val roleTag = jsGraph.typeNodes(jsGraph.root).metadata.role.toOption.map(_.tag)

          assertTrue(roleTag == Some("unstructured-text"))
        },
        test("schemaGraphFromJson decodes serde-shaped schema node metadata") {
          val graphJson =
            """{"root":{"kind":"variant","value":{"cases":[{"name":"inline","payload":{"kind":"text","value":{"restrictions":{}}}},{"name":"url","payload":{"kind":"url","value":{"restrictions":{}}}}],"metadata":{"role":{"tag":"unstructured-text"}}}}}"""
          val graph = SchemaRpcCodec.schemaGraphFromJson(graphJson)
          assertTrue(graph.root.metadata.role == Some(Role.UnstructuredText))
        },
        test("schemaGraphFromJson keeps other role distinct from dedicated unstructured tag") {
          val graphJson =
            """{"root":{"kind":"string","value":{"metadata":{"role":{"tag":"other","value":"unstructured-text"}}}}}"""
          val graph   = SchemaRpcCodec.schemaGraphFromJson(graphJson)
          val jsGraph = SchemaWireInterop.graphToJs(SchemaWire.schemaGraphToWit(graph))
          val role    = jsGraph.typeNodes(jsGraph.root).metadata.role.toOption
          assertTrue(
            graph.root.metadata.role == Some(Role.Other("unstructured-text")),
            role.map(_.tag) == Some("other")
          )
        },
        test("schemaGraphFromJson preserves large quantity mantissas") {
          val graphJson =
            """{"root":{"kind":"quantity","value":{"spec":{"baseUnit":"token","min":{"mantissa":"9007199254740993","scale":0,"unit":"token"}}}}}"""
          val graph = SchemaRpcCodec.schemaGraphFromJson(graphJson)
          assertTrue(
            graph.root == SchemaType(
              SchemaTypeBody.QuantityType(
                QuantitySpec("token", min = Some(QuantityValue(9007199254740993L, 0, "token")))
              )
            )
          )
        }
      )
    )
}
