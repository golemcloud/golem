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

package golem.schema

import golem.schema.wire._
import zio.test.Assertion._
import zio.test._

import scala.collection.immutable.ListMap
import scala.util.Try

object SchemaModelSpec extends ZIOSpecDefault {

  // A schema graph exercising every `SchemaTypeBody` arm, including a `ref` to a
  // registered nominal definition and metadata on selected nodes.
  private def kitchenSinkTypeGraph: SchemaGraph = {
    import SchemaTypeBody._
    val b = new SchemaBuilder

    val pointRef =
      b.register(
        "ns.point",
        () => t.record(List(t.field("x", t.s32), t.field("y", t.s32))),
        Some("Point")
      )

    val fields = List(
      NamedFieldType("ref", pointRef),
      NamedFieldType("bool", t.bool),
      NamedFieldType("s8", t.s8),
      NamedFieldType("s16", t.s16),
      NamedFieldType("s32", t.s32),
      NamedFieldType("s64", t.s64),
      NamedFieldType("u8", t.u8),
      NamedFieldType("u16", t.u16),
      NamedFieldType("u32", t.u32),
      NamedFieldType("u64", t.u64),
      NamedFieldType("f32", t.f32),
      NamedFieldType("f64", t.f64),
      NamedFieldType("char", t.char),
      NamedFieldType(
        "string",
        t.string,
        MetadataEnvelope(doc = Some("a string"), aliases = List("str"), examples = List("\"hi\""))
      ),
      NamedFieldType("record", t.record(List(t.field("inner", t.bool)))),
      NamedFieldType(
        "variant",
        t.variant(List(VariantCaseType("none"), VariantCaseType("some", Some(t.s32))))
      ),
      NamedFieldType("enum", t.`enum`(List("red", "green", "blue"))),
      NamedFieldType("flags", t.flags(List("a", "b", "c"))),
      NamedFieldType("tuple", t.tuple(List(t.s32, t.string))),
      NamedFieldType("list", t.list(t.s32)),
      NamedFieldType("fixedList", t.fixedList(t.u8, 4)),
      NamedFieldType("map", t.map(t.string, t.s32)),
      NamedFieldType("option", t.option(t.string)),
      NamedFieldType("result", t.result(Some(t.s32), Some(t.string))),
      NamedFieldType(
        "text",
        SchemaType(TextType(TextRestrictions(languages = Some(List("en")), maxLength = Some(10))))
      ),
      NamedFieldType("binary", SchemaType(BinaryType(BinaryRestrictions(mimeTypes = Some(List("image/png")))))),
      NamedFieldType("path", SchemaType(PathType(PathSpec(PathDirection.Input, PathKind.File)))),
      NamedFieldType("url", SchemaType(UrlType(UrlRestrictions(allowedSchemes = Some(List("https")))))),
      NamedFieldType("datetime", t.datetime),
      NamedFieldType("duration", t.duration),
      NamedFieldType(
        "quantity",
        SchemaType(QuantityType(QuantitySpec("kg", List("kg", "g"), Some(QuantityValue(0L, 0, "kg")))))
      ),
      NamedFieldType(
        "union",
        SchemaType(
          UnionType(
            List(
              UnionBranch("ssh", t.string, DiscriminatorRule.Prefix("ssh://")),
              UnionBranch(
                "obj",
                t.record(List(t.field("kind", t.string))),
                DiscriminatorRule.FieldEquals(FieldDiscriminator("kind", Some("obj")))
              )
            )
          )
        )
      ),
      NamedFieldType("secret", SchemaType(SecretType(SecretSpec(t.string, Some("api-key"))))),
      NamedFieldType("quotaToken", SchemaType(QuotaTokenType(QuotaTokenSpec(Some("tokens"))))),
      NamedFieldType("future", SchemaType(FutureType(Some(t.s32)))),
      NamedFieldType("stream", SchemaType(StreamType(None)))
    )

    b.buildGraph(t.record(fields))
  }

  // A value tree exercising every `SchemaValue` arm. Structurally independent of
  // the schema (the wire codec for values does not validate against a schema).
  private def kitchenSinkValue: SchemaValue = {
    import SchemaValue._
    RecordValue(
      List(
        BoolValue(true),
        S8Value(-1),
        S16Value(-2),
        S32Value(-3),
        S64Value(-4L),
        U8Value(200),
        U16Value(40000),
        U32Value(4000000000L),
        U64Value(-1L), // raw bits of u64::MAX
        F32Value(1.5f),
        F64Value(2.5d),
        CharValue('z'.toInt),
        StringValue("hello"),
        RecordValue(List(BoolValue(false))),
        VariantValue(1, Some(S32Value(7))),
        EnumValue(2),
        FlagsValue(List(true, false, true)),
        TupleValue(List(S32Value(1), StringValue("x"))),
        ListValue(List(S32Value(1), S32Value(2))),
        FixedListValue(List(U8Value(1), U8Value(2), U8Value(3), U8Value(4))),
        MapValue(List(SchemaMapEntry(StringValue("k"), S32Value(9)))),
        OptionValue(Some(StringValue("present"))),
        OptionValue(None),
        ResultValue(SchemaResult.Ok(Some(S32Value(0)))),
        ResultValue(SchemaResult.Err(None)),
        TextValue("bonjour", Some("fr")),
        BinaryValue(Vector[Byte](1, 2, 3, -1), Some("image/png")),
        PathValue("/tmp/x"),
        UrlValue("https://example.com"),
        DatetimeValue(Datetime(1700000000L, 500)),
        DurationValue(-123456789L),
        QuantityValueNode(QuantityValue(1500L, 3, "kg")),
        UnionValue("ssh", StringValue("ssh://host")),
        SecretValue(GuestSecretHandle.fromRaw("secret-handle-1")),
        QuotaTokenHandle(GuestQuotaTokenHandle.fromRaw("quota-token-handle-1"))
      )
    )
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("SchemaModelSpec")(
      suite("SchemaBuilder")(
        test("register reserves before building so self-recursive types close to a ref") {
          import SchemaTypeBody._
          val b = new SchemaBuilder
          // A cons-list: list { head: s32, tail: option<list> }
          val listRef = b.register(
            "ns.list",
            () => t.record(List(t.field("head", t.s32), t.field("tail", t.option(b.ref("ns.list")))))
          )
          val graph     = b.buildGraph(listRef)
          val tailField = graph.defs("ns.list").body.body.asInstanceOf[RecordType].fields(1)
          val inner     = tailField.body.body.asInstanceOf[OptionType].element.body
          assertTrue(
            graph.root.body == RefType("ns.list"),
            graph.defs.contains("ns.list"),
            inner == RefType("ns.list")
          )
        },
        test("mutually recursive types both close to refs") {
          import SchemaTypeBody._
          val b    = new SchemaBuilder
          val aRef = b.register(
            "ns.a",
            () => t.record(List(t.field("b", b.register("ns.b", () => t.record(List(t.field("a", b.ref("ns.a"))))))))
          )
          val graph = b.buildGraph(aRef)
          val aToB  = graph.defs("ns.a").body.body.asInstanceOf[RecordType].fields(0).body.body
          val bToA  = graph.defs("ns.b").body.body.asInstanceOf[RecordType].fields(0).body.body
          assertTrue(graph.defs.size == 2, aToB == RefType("ns.b"), bToA == RefType("ns.a"))
        },
        test("register is idempotent: a second register of the same id does not rebuild") {
          val b                   = new SchemaBuilder
          var builds              = 0
          def build(): SchemaType = { builds += 1; t.record(List(t.field("x", t.s32))) }
          val r1                  = b.register("ns.x", () => build())
          val r2                  = b.register("ns.x", () => build())
          assertTrue(builds == 1, r1 == r2, r1.body == SchemaTypeBody.RefType("ns.x"))
        },
        test("finish fails if a reserved id was never committed") {
          val b = new SchemaBuilder
          b.reserve("ns.dangling")
          val res = Try(b.finish())
          assert(res)(isFailure(isSubtype[SchemaEncodeError](anything)))
        }
      ),
      suite("graph merge")(
        test("mergeGraphDefs deduplicates identical defs across graphs") {
          val g1     = SchemaBuilder.graphOf(b => b.register("ns.p", () => t.record(List(t.field("x", t.s32)))))
          val g2     = SchemaBuilder.graphOf(b => b.register("ns.p", () => t.record(List(t.field("x", t.s32)))))
          val merged = SchemaBuilder.mergeGraphDefs(List(g1, g2))
          assertTrue(merged.size == 1, merged.contains("ns.p"))
        },
        test("mergeGraphDefs rejects conflicting same-id bodies") {
          val g1  = SchemaBuilder.graphOf(b => b.register("ns.p", () => t.record(List(t.field("x", t.s32)))))
          val g2  = SchemaBuilder.graphOf(b => b.register("ns.p", () => t.record(List(t.field("x", t.string)))))
          val res = Try(SchemaBuilder.mergeGraphDefs(List(g1, g2)))
          assert(res)(isFailure(isSubtype[SchemaConflictError](anything)))
        },
        test("mergeAgentGraphs keeps roots in input order and shares defs") {
          val g1     = SchemaBuilder.graphOf(b => t.list(b.register("ns.p", () => t.record(List(t.field("x", t.s32))))))
          val g2     = SchemaBuilder.graphOf(b => t.option(b.register("ns.p", () => t.record(List(t.field("x", t.s32))))))
          val merged = SchemaBuilder.mergeAgentGraphs(List(g1, g2))
          assertTrue(
            merged.defs.size == 1,
            merged.roots.length == 2,
            merged.roots(0) == g1.root,
            merged.roots(1) == g2.root
          )
        }
      ),
      suite("type graph wire roundtrip")(
        test("kitchen-sink type graph round-trips through the flat WIT carrier") {
          val graph    = kitchenSinkTypeGraph
          val wit      = SchemaWire.schemaGraphToWit(graph)
          val restored = SchemaWire.schemaGraphFromWit(wit)
          assertTrue(restored == graph)
        },
        test("flattened defs are deterministically sorted by id regardless of insertion order") {
          val b = new SchemaBuilder
          b.register("z.last", () => t.record(List(t.field("v", t.s32))))
          b.register("a.first", () => t.record(List(t.field("v", t.s32))))
          b.register("m.middle", () => t.record(List(t.field("v", t.s32))))
          val wit = SchemaWire.schemaGraphToWit(b.buildGraph(t.bool))
          assertTrue(wit.defs.map(_.id) == Vector("a.first", "m.middle", "z.last"))
        },
        test("ref bodies flatten to def indices that match the sorted def order") {
          val b = new SchemaBuilder
          b.register("z.point", () => t.record(List(t.field("x", t.s32))))
          val ref      = b.register("a.point", () => t.record(List(t.field("y", t.s32))))
          val wit      = SchemaWire.schemaGraphToWit(b.buildGraph(ref))
          val rootBody = wit.typeNodes(wit.root).body
          // "a.point" sorts before "z.point", so its def index is 0.
          assertTrue(rootBody == WitSchemaTypeBody.RefType(0))
        }
      ),
      suite("value tree wire roundtrip")(
        test("kitchen-sink value round-trips through the flat WIT carrier") {
          val value    = kitchenSinkValue
          val wit      = SchemaWire.schemaValueToWit(value)
          val restored = SchemaWire.schemaValueFromWit(wit)
          assertTrue(restored == value)
        },
        test("binary values compare structurally after a roundtrip") {
          val v        = SchemaValue.BinaryValue(Vector[Byte](0, 127, -128, -1), None)
          val restored = SchemaWire.schemaValueFromWit(SchemaWire.schemaValueToWit(v))
          assertTrue(restored == v)
        },
        test("u64 max raw bits survive as a Long") {
          val v        = SchemaValue.U64Value(-1L)
          val restored = SchemaWire.schemaValueFromWit(SchemaWire.schemaValueToWit(v))
          assertTrue(restored == SchemaValue.U64Value(-1L))
        }
      ),
      suite("typed schema value roundtrip")(
        test("typed value (graph + value) round-trips") {
          val tv       = TypedSchemaValue(kitchenSinkTypeGraph, kitchenSinkValue)
          val restored = SchemaWire.typedSchemaValueFromWit(SchemaWire.typedSchemaValueToWit(tv))
          assertTrue(restored == tv)
        }
      ),
      suite("error paths")(
        test("encoding a graph with a dangling ref fails with SchemaEncodeError") {
          val graph = SchemaGraph(ListMap.empty[String, SchemaTypeDef], t.ref("ns.missing"))
          val res   = Try(SchemaWire.schemaGraphToWit(graph))
          assert(res)(isFailure(isSubtype[SchemaEncodeError](anything)))
        },
        test("decoding a value tree with an out-of-range root index fails") {
          val bad = WitSchemaValueTree(Vector.empty, 0)
          val res = Try(SchemaWire.schemaValueFromWit(bad))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything)))
        },
        test("decoding a value tree with a cyclic reference fails") {
          // node 0 is an option pointing at itself
          val cyclic = WitSchemaValueTree(Vector(WitSchemaValueNode.OptionValue(Some(0))), 0)
          val res    = Try(SchemaWire.schemaValueFromWit(cyclic))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything)))
        },
        test("decoding a graph with a def index out of range fails") {
          val bad = WitSchemaGraph(
            typeNodes = Vector(WitSchemaTypeNode(WitSchemaTypeBody.RefType(5), MetadataEnvelope.empty)),
            defs = Vector.empty,
            root = 0
          )
          val res = Try(SchemaWire.schemaGraphFromWit(bad))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything)))
        },
        test("decoding a graph with duplicate def ids fails") {
          val node = WitSchemaTypeNode(WitSchemaTypeBody.BoolType, MetadataEnvelope.empty)
          val bad  = WitSchemaGraph(
            typeNodes = Vector(node),
            defs = Vector(WitSchemaTypeDef("dup", None, 0), WitSchemaTypeDef("dup", None, 0)),
            root = 0
          )
          val res = Try(SchemaWire.schemaGraphFromWit(bad))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything)))
        },
        test("decoding a type graph with a structural type-node cycle fails") {
          // node 0 is a list whose element is itself (a raw index cycle, not a def ref)
          val cyclic = WitSchemaGraph(
            typeNodes = Vector(WitSchemaTypeNode(WitSchemaTypeBody.ListType(0), MetadataEnvelope.empty)),
            defs = Vector.empty,
            root = 0
          )
          val res = Try(SchemaWire.schemaGraphFromWit(cyclic))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything)))
        },
        test("decoding a type graph with an out-of-range root index fails") {
          val bad = WitSchemaGraph(typeNodes = Vector.empty, defs = Vector.empty, root = 0)
          val res = Try(SchemaWire.schemaGraphFromWit(bad))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything)))
        },
        test("decoding a type graph with an out-of-range child index fails") {
          // a record whose only field body points at a non-existent node
          val bad = WitSchemaGraph(
            typeNodes = Vector(
              WitSchemaTypeNode(
                WitSchemaTypeBody.RecordType(Vector(WitNamedFieldType("x", 9, MetadataEnvelope.empty))),
                MetadataEnvelope.empty
              )
            ),
            defs = Vector.empty,
            root = 0
          )
          val res = Try(SchemaWire.schemaGraphFromWit(bad))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything)))
        },
        test("decoding a value tree with an out-of-range child index fails") {
          val bad = WitSchemaValueTree(Vector(WitSchemaValueNode.RecordValue(Vector(9))), 0)
          val res = Try(SchemaWire.schemaValueFromWit(bad))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything)))
        }
      ),
      suite("quota-token handle affine semantics")(
        test("a single quota-token handle round-trips preserving handle identity") {
          import SchemaValue._
          val h          = GuestQuotaTokenHandle.fromRaw("raw-1")
          val value      = QuotaTokenHandle(h)
          val restored   = SchemaWire.schemaValueFromWit(SchemaWire.schemaValueToWit(value))
          val sameHandle = restored match {
            case QuotaTokenHandle(h2) => h2 eq h
            case _                    => false
          }
          assertTrue(restored == value, sameHandle, h.isPresent)
        },
        test("encoding an already-transferred quota-token handle fails with SchemaEncodeError") {
          import SchemaValue._
          val h = GuestQuotaTokenHandle.fromRaw("raw-1")
          h.take()
          val res = Try(SchemaWire.schemaValueToWit(QuotaTokenHandle(h)))
          assert(res)(isFailure(isSubtype[SchemaEncodeError](anything)))
        },
        test("encoding the same quota-token handle twice in one tree fails with SchemaEncodeError") {
          import SchemaValue._
          val h     = GuestQuotaTokenHandle.fromRaw("raw-1")
          val value = RecordValue(List(QuotaTokenHandle(h), QuotaTokenHandle(h)))
          val res   = Try(SchemaWire.schemaValueToWit(value))
          assert(res)(isFailure(isSubtype[SchemaEncodeError](anything)))
        },
        test("decoding a tree that references one quota-token handle node twice fails and drains it") {
          val h   = GuestQuotaTokenHandle.fromRaw("raw-1")
          val bad = WitSchemaValueTree(
            Vector(
              WitSchemaValueNode.RecordValue(Vector(1, 1)),
              WitSchemaValueNode.QuotaTokenHandle(h)
            ),
            0
          )
          val res = Try(SchemaWire.schemaValueFromWit(bad))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything))) && assertTrue(!h.isPresent)
        },
        test("decoding a tree with an unreferenced quota-token handle node fails and drains it") {
          val h   = GuestQuotaTokenHandle.fromRaw("raw-1")
          val bad = WitSchemaValueTree(
            Vector(
              WitSchemaValueNode.BoolValue(true),
              WitSchemaValueNode.QuotaTokenHandle(h)
            ),
            0
          )
          val res = Try(SchemaWire.schemaValueFromWit(bad))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything))) && assertTrue(!h.isPresent)
        },
        test("decoding a valid root plus an unreachable handle node fails and drains both handles") {
          // node 0 is a record referencing node 1 (a valid, reachable handle);
          // node 2 is a second handle unreachable from the root.
          val reachable   = GuestQuotaTokenHandle.fromRaw("raw-reachable")
          val unreachable = GuestQuotaTokenHandle.fromRaw("raw-unreachable")
          val bad         = WitSchemaValueTree(
            Vector(
              WitSchemaValueNode.RecordValue(Vector(1)),
              WitSchemaValueNode.QuotaTokenHandle(reachable),
              WitSchemaValueNode.QuotaTokenHandle(unreachable)
            ),
            0
          )
          val res = Try(SchemaWire.schemaValueFromWit(bad))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything))) &&
          assertTrue(!reachable.isPresent, !unreachable.isPresent)
        },
        test("decoding fails atomically when a sibling is invalid, draining an already-reached handle") {
          // Tuple([quota-handle, list-with-out-of-range-child]): the handle is
          // reached first, then the sibling's child index is rejected. The handle
          // must be released, not left live in a discarded partial value.
          val h   = GuestQuotaTokenHandle.fromRaw("raw-1")
          val bad = WitSchemaValueTree(
            Vector(
              WitSchemaValueNode.TupleValue(Vector(1, 2)),
              WitSchemaValueNode.QuotaTokenHandle(h),
              WitSchemaValueNode.ListValue(Vector(99))
            ),
            0
          )
          val res = Try(SchemaWire.schemaValueFromWit(bad))
          assert(res)(isFailure(isSubtype[SchemaDecodeError](anything))) && assertTrue(!h.isPresent)
        }
      ),
      suite("GraphEncoder multi-root (agent carrier use case)")(
        test("encodes several roots into one shared pool with a placeholder finish root") {
          val g1 = SchemaBuilder.graphOf(b => t.list(b.register("ns.p", () => t.record(List(t.field("x", t.s32))))))
          val g2 =
            SchemaBuilder.graphOf(b => t.option(b.register("ns.p", () => t.record(List(t.field("x", t.s32))))))
          val merged = SchemaBuilder.mergeAgentGraphs(List(g1, g2))

          val enc        = new GraphEncoder(merged.defs)
          val r1         = enc.encodeType(merged.roots(0))
          val r2         = enc.encodeType(merged.roots(1))
          val wit        = enc.finish()
          val poolSize   = wit.typeNodes.length
          val rootIsRec  = wit.typeNodes(wit.root).body == WitSchemaTypeBody.RecordType(Vector.empty)
          val r1IsList   = wit.typeNodes(r1).body.isInstanceOf[WitSchemaTypeBody.ListType]
          val r2IsOption = wit.typeNodes(r2).body.isInstanceOf[WitSchemaTypeBody.OptionType]
          assertTrue(
            wit.defs.length == 1,
            wit.defs(0).id == "ns.p",
            r1 >= 0 && r1 < poolSize,
            r2 >= 0 && r2 < poolSize,
            wit.root >= 0 && wit.root < poolSize,
            rootIsRec,
            r1IsList,
            r2IsOption
          )
        }
      ),
      suite("edge scalars and remaining enum/discriminator arms")(
        test("unsigned and boundary scalars survive a value roundtrip") {
          import SchemaValue._
          val values = List[SchemaValue](
            U32Value(4294967295L),
            U64Value(java.lang.Long.MIN_VALUE),
            U64Value(java.lang.Long.MAX_VALUE),
            DatetimeValue(Datetime(java.lang.Long.MIN_VALUE, 999999999)),
            DurationValue(java.lang.Long.MIN_VALUE)
          )
          val tuple    = TupleValue(values)
          val restored = SchemaWire.schemaValueFromWit(SchemaWire.schemaValueToWit(tuple))
          assertTrue(restored == tuple)
        },
        test("all discriminator rules and path/role metadata arms survive a type roundtrip") {
          import SchemaTypeBody._
          val union = SchemaType(
            UnionType(
              List(
                UnionBranch("p", t.string, DiscriminatorRule.Prefix("a")),
                UnionBranch("s", t.string, DiscriminatorRule.Suffix("z")),
                UnionBranch("c", t.string, DiscriminatorRule.Contains("m")),
                UnionBranch("r", t.string, DiscriminatorRule.Regex("^x.*$")),
                UnionBranch(
                  "fe",
                  t.record(List(t.field("kind", t.string))),
                  DiscriminatorRule.FieldEquals(FieldDiscriminator("kind", None))
                ),
                UnionBranch("fa", t.record(List(t.field("k", t.string))), DiscriminatorRule.FieldAbsent("missing"))
              )
            )
          )
          val root = t.record(
            List(
              NamedFieldType("union", union),
              NamedFieldType("pOut", SchemaType(PathType(PathSpec(PathDirection.Output, PathKind.Directory)))),
              NamedFieldType("pInOut", SchemaType(PathType(PathSpec(PathDirection.InOut, PathKind.Any)))),
              NamedFieldType("mm", t.bool, MetadataEnvelope(role = Some(Role.Multimodal))),
              NamedFieldType("other", t.bool, MetadataEnvelope(role = Some(Role.Other("custom")))),
              NamedFieldType("dep", t.bool, MetadataEnvelope(deprecated = Some("use something else")))
            )
          )
          val graph    = SchemaGraph(ListMap.empty[String, SchemaTypeDef], root)
          val restored = SchemaWire.schemaGraphFromWit(SchemaWire.schemaGraphToWit(graph))
          assertTrue(restored == graph)
        }
      )
    )
}
