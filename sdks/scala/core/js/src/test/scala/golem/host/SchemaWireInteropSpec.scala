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

package golem.host

import golem.schema._
import golem.schema.wire._
import golem.schema.wire.WitSchemaTypeBody._
import golem.schema.wire.WitSchemaValueNode._
import golem.host.js.schema.{JsSchemaTypeBody, JsSchemaValueNode, JsSchemaValueTree}
import zio.test._

import scala.scalajs.js
import scala.scalajs.js.typedarray.Uint8Array

/**
 * Exhaustive `Wit* -> Js* -> Wit*` round-trip for the v2
 * `golem:core/types@2.0.0` flat carrier. Because the `Wit*` ADT is plain Scala
 * case classes, identity after a full JS bounce proves every facade field name
 * / tag string in [[golem.host.SchemaWireInterop]] agrees with the encoder it
 * pairs with (a wrong JS field name reads back as `undefined`, which would
 * change the value and fail equality).
 */
object SchemaWireInteropSpec extends ZIOSpecDefault {

  // --- metadata variations -------------------------------------------------

  private val mdFull =
    MetadataEnvelope(Some("doc text"), List("a1", "a2"), List("ex1", "ex2"), Some("use X"), Some(Role.Multimodal))
  private val mdOther = MetadataEnvelope(Some("d2"), List("b"), Nil, None, Some(Role.Other("custom-role")))
  private val md0     = MetadataEnvelope.empty

  // --- spec variations -----------------------------------------------------

  private val textFull = TextRestrictions(Some(List("en", "hu")), Some(1), Some(99), Some("a.*z"))
  private val textNone = TextRestrictions.empty

  private val binFull = BinaryRestrictions(Some(List("image/png")), Some(0), Some(1024))
  private val binNone = BinaryRestrictions.empty

  private val pathFull = PathSpec(PathDirection.InOut, PathKind.Directory, Some(List("text/plain")), Some(List("txt")))
  private val pathIn   = PathSpec(PathDirection.Input, PathKind.File, None, None)
  private val pathOut  = PathSpec(PathDirection.Output, PathKind.Any, None, None)

  private val urlFull = UrlRestrictions(Some(List("https")), Some(List("example.com")))
  private val urlNone = UrlRestrictions.empty

  private val qvalue    = QuantityValue(12345L, 3, "kg")
  private val quantFull = QuantitySpec("kg", List("kg", "g", "mg"), Some(QuantityValue(0L, 0, "kg")), Some(qvalue))
  private val quantNone = QuantitySpec("s", Nil, None, None)

  private val discriminators: Vector[DiscriminatorRule] = Vector(
    DiscriminatorRule.Prefix("ssh://"),
    DiscriminatorRule.Suffix(".tar.gz"),
    DiscriminatorRule.Contains("mid"),
    DiscriminatorRule.Regex("^a.*$"),
    DiscriminatorRule.FieldEquals(FieldDiscriminator("kind", Some("circle"))),
    DiscriminatorRule.FieldEquals(FieldDiscriminator("kind", None)),
    DiscriminatorRule.FieldAbsent("legacy")
  )

  private val unionSpec = WitUnionSpec(
    discriminators.zipWithIndex.map { case (d, i) =>
      WitUnionBranch(s"branch$i", i, d, if (i % 2 == 0) mdFull else md0)
    }
  )

  // --- type nodes: one (or more) per WitSchemaTypeBody case ----------------

  private val typeBodies: Vector[WitSchemaTypeBody] = Vector(
    RefType(0),
    BoolType,
    S8Type,
    S16Type,
    S32Type,
    S64Type,
    U8Type,
    U16Type,
    U32Type,
    U64Type,
    F32Type,
    F64Type,
    CharType,
    StringType,
    RecordType(Vector(WitNamedFieldType("x", 1, mdFull), WitNamedFieldType("y", 2, md0))),
    VariantType(Vector(WitVariantCaseType("Some", Some(3), mdOther), WitVariantCaseType("None", None, md0))),
    EnumType(Vector("Red", "Green", "Blue")),
    FlagsType(Vector("read", "write")),
    TupleType(Vector(1, 2, 3)),
    ListType(4),
    FixedListType(WitFixedListSpec(5, 7)),
    MapType(WitMapSpec(6, 7)),
    OptionType(8),
    ResultType(WitResultSpec(Some(9), Some(10))),
    ResultType(WitResultSpec(Some(9), None)),
    ResultType(WitResultSpec(None, Some(10))),
    ResultType(WitResultSpec(None, None)),
    TextType(textFull),
    TextType(textNone),
    BinaryType(binFull),
    BinaryType(binNone),
    PathType(pathFull),
    PathType(pathIn),
    PathType(pathOut),
    UrlType(urlFull),
    UrlType(urlNone),
    DatetimeType,
    DurationType,
    QuantityType(quantFull),
    QuantityType(quantNone),
    UnionType(unionSpec),
    SecretType(WitSecretSpec(0, Some("api-key"))),
    SecretType(WitSecretSpec(0, None)),
    QuotaTokenType(QuotaTokenSpec(Some("res"))),
    QuotaTokenType(QuotaTokenSpec(None)),
    FutureType(Some(11)),
    FutureType(None),
    StreamType(Some(12)),
    StreamType(None)
  )

  private val typeNodes: Vector[WitSchemaTypeNode] =
    typeBodies.zipWithIndex.map { case (b, i) =>
      WitSchemaTypeNode(b, if (i % 3 == 0) mdFull else if (i % 3 == 1) mdOther else md0)
    }

  private val defs: Vector[WitSchemaTypeDef] = Vector(
    WitSchemaTypeDef("myapp.Point", Some("Point"), 14),
    WitSchemaTypeDef("myapp.Anon", None, 15)
  )

  private val graph = WitSchemaGraph(typeNodes, defs, root = 14)

  // --- value nodes: one (or more) per WitSchemaValueNode case ---------------

  private val datetime = Datetime(1_700_000_000L, 123_456_789)

  // Quota-token handle nodes are intentionally excluded from this structural
  // round-trip vector: an owned `quota-token` handle is affine, so encoding
  // consumes it and decoding wraps a fresh handle, which cannot compare equal by
  // identity. Their interop is covered by dedicated tests below.
  private val valueNodes: Vector[WitSchemaValueNode] = Vector(
    BoolValue(true),
    S8Value(-8),
    S16Value(-16),
    S32Value(-32),
    S64Value(-64L),
    U8Value(200),
    U16Value(60000),
    U32Value(4000000000L),
    U64Value(-1L), // raw bits
    F32Value(1.5f),
    F64Value(2.5d),
    CharValue('q'.toInt),
    StringValue("hello"),
    RecordValue(Vector(0, 1, 2)),
    VariantValue(WitVariantValuePayload(1, Some(3))),
    VariantValue(WitVariantValuePayload(0, None)),
    EnumValue(2),
    FlagsValue(Vector(true, false, true)),
    TupleValue(Vector(1, 2)),
    ListValue(Vector(3, 4, 5)),
    FixedListValue(Vector(6, 7)),
    MapValue(Vector(WitMapEntry(0, 1), WitMapEntry(2, 3))),
    OptionValue(Some(8)),
    OptionValue(None),
    ResultValue(WitResultValuePayload.OkValue(Some(9))),
    ResultValue(WitResultValuePayload.OkValue(None)),
    ResultValue(WitResultValuePayload.ErrValue(Some(10))),
    ResultValue(WitResultValuePayload.ErrValue(None)),
    TextValue(WitTextValuePayload("note", Some("en"))),
    TextValue(WitTextValuePayload("note2", None)),
    BinaryValue(WitBinaryValuePayload(Vector[Byte](1, 2, 3, -1), Some("image/png"))),
    BinaryValue(WitBinaryValuePayload(Vector.empty, None)),
    PathValue("/tmp/x"),
    UrlValue("https://example.com"),
    DatetimeValue(datetime),
    DurationValue(WitDurationValuePayload(987654321L)),
    QuantityValueNode(qvalue),
    UnionValue(WitUnionValuePayload("branch0", 1)),
    // --- numeric / boundary samples -----------------------------------------
    CharValue(0x1f600),    // astral-plane code point (emoji), needs surrogate pair
    U32Value(4294967295L), // u32 max
    S64Value(Long.MinValue),
    S64Value(Long.MaxValue),
    U64Value(Long.MinValue), // raw bits => unsigned 2^63
    DurationValue(WitDurationValuePayload(Long.MinValue)),
    QuantityValueNode(QuantityValue(Long.MinValue, -2147483648, "u"))
  )

  private val valueTree = WitSchemaValueTree(valueNodes, root = 0)

  private val typed = WitTypedSchemaValue(graph, valueTree)

  override def spec: Spec[TestEnvironment, Any] =
    suite("SchemaWireInteropSpec")(
      test("schema graph round-trips Wit -> Js -> Wit") {
        assertTrue(SchemaWireInterop.graphFromJs(SchemaWireInterop.graphToJs(graph)) == graph)
      },
      test("schema value tree round-trips Wit -> Js -> Wit") {
        assertTrue(SchemaWireInterop.valueTreeFromJs(SchemaWireInterop.valueTreeToJs(valueTree)) == valueTree)
      },
      test("typed schema value round-trips Wit -> Js -> Wit (all cases)") {
        assertTrue(SchemaWireInterop.typedFromJs(SchemaWireInterop.typedToJs(typed)) == typed)
      },
      test("per-type-body round-trips individually") {
        val results = typeBodies.map { b =>
          val g = WitSchemaGraph(Vector(WitSchemaTypeNode(b, md0)), Vector.empty, 0)
          SchemaWireInterop.graphFromJs(SchemaWireInterop.graphToJs(g)) == g
        }
        assertTrue(results.forall(identity))
      },
      test("per-value-node round-trips individually") {
        val results = valueNodes.map { n =>
          val v = WitSchemaValueTree(Vector(n), 0)
          SchemaWireInterop.valueTreeFromJs(SchemaWireInterop.valueTreeToJs(v)) == v
        }
        assertTrue(results.forall(identity))
      },
      test("secret handle: encode moves the owned resource and consumes the handle") {
        val raw    = js.Dynamic.literal(marker = 43).asInstanceOf[js.Any]
        val handle = GuestSecretHandle.fromRaw(raw)
        val jsNode =
          SchemaWireInterop.valueTreeToJs(WitSchemaValueTree(Vector(SecretValue(handle)), 0)).valueNodes(0)
        assertTrue(
          jsNode.tag == "secret-value",
          rawVal(jsNode).asInstanceOf[AnyRef] eq raw.asInstanceOf[AnyRef],
          !handle.isPresent
        )
      },
      test("secret handle: decode wraps the owned resource in a fresh present handle") {
        val raw    = js.Dynamic.literal(marker = 8).asInstanceOf[js.Any]
        val jsTree = SchemaWireInterop.valueTreeToJs(
          WitSchemaValueTree(Vector(SecretValue(GuestSecretHandle.fromRaw(raw))), 0)
        )
        val decoded = SchemaWireInterop.valueTreeFromJs(jsTree)
        decoded.valueNodes(0) match {
          case SecretValue(h) =>
            assertTrue(h.isPresent, h.take().exists(_.asInstanceOf[AnyRef] eq raw.asInstanceOf[AnyRef]))
          case other => assertTrue(false).label(s"expected SecretValue, got $other")
        }
      },
      test("secret handle: inbound decode rejects two nodes carrying the same raw resource") {
        val raw = js.Dynamic.literal(marker = 44).asInstanceOf[js.Any]
        val jsTree = JsSchemaValueTree(
          js.Array(
            JsSchemaValueNode.recordValue(js.Array(1, 2)),
            JsSchemaValueNode.secretValue(raw),
            JsSchemaValueNode.secretValue(raw)
          ),
          0
        )
        val wit    = SchemaWireInterop.valueTreeFromJs(jsTree)
        val result = scala.util.Try(SchemaWire.schemaValueFromWit(wit))
        result match {
          case scala.util.Failure(_: SchemaDecodeError) => assertTrue(true)
          case other                                    => assertTrue(false).label(s"expected SchemaDecodeError, got $other")
        }
      },
      test("secret handle: encode is atomic — a sibling that fails leaves the handle untouched") {
        val raw    = js.Dynamic.literal(marker = 100).asInstanceOf[js.Any]
        val handle = GuestSecretHandle.fromRaw(raw)
        val tree   = WitSchemaValueTree(
          Vector(
            TupleValue(Vector(1, 2)),
            SecretValue(handle),
            DatetimeValue(Datetime(0L, -1))
          ),
          0
        )
        val result = scala.util.Try(SchemaWireInterop.valueTreeToJs(tree))
        assertTrue(result.isFailure, handle.isPresent)
      },
      test("quota-token handle: encode moves the owned resource and consumes the handle") {
        val raw    = js.Dynamic.literal(marker = 42).asInstanceOf[js.Any]
        val handle = GuestQuotaTokenHandle.fromRaw(raw)
        val jsNode =
          SchemaWireInterop.valueTreeToJs(WitSchemaValueTree(Vector(QuotaTokenHandle(handle)), 0)).valueNodes(0)
        assertTrue(
          jsNode.tag == "quota-token-handle",
          rawVal(jsNode).asInstanceOf[AnyRef] eq raw.asInstanceOf[AnyRef],
          !handle.isPresent
        )
      },
      test("quota-token handle: decode wraps the owned resource in a fresh present handle") {
        val raw    = js.Dynamic.literal(marker = 7).asInstanceOf[js.Any]
        val jsTree = SchemaWireInterop.valueTreeToJs(
          WitSchemaValueTree(Vector(QuotaTokenHandle(GuestQuotaTokenHandle.fromRaw(raw))), 0)
        )
        val decoded = SchemaWireInterop.valueTreeFromJs(jsTree)
        decoded.valueNodes(0) match {
          case QuotaTokenHandle(h) =>
            assertTrue(h.isPresent, h.take().exists(_.asInstanceOf[AnyRef] eq raw.asInstanceOf[AnyRef]))
          case other => assertTrue(false).label(s"expected QuotaTokenHandle, got $other")
        }
      },
      test("quota-token handle: encode is atomic — a sibling that fails leaves the handle untouched") {
        val raw    = js.Dynamic.literal(marker = 99).asInstanceOf[js.Any]
        val handle = GuestQuotaTokenHandle.fromRaw(raw)
        // Tuple([quota-token-handle, datetime-with-invalid-nanoseconds]): the
        // datetime would be rejected by the boundary, so the preflight must fail
        // before the affine handle is moved out of its cell.
        val tree = WitSchemaValueTree(
          Vector(
            TupleValue(Vector(1, 2)),
            QuotaTokenHandle(handle),
            DatetimeValue(Datetime(0L, -1))
          ),
          0
        )
        val result = scala.util.Try(SchemaWireInterop.valueTreeToJs(tree))
        assertTrue(result.isFailure, handle.isPresent)
      },
      // The round-trip tests above only prove encode/decode self-consistency. These
      // smoke tests assert the *raw* emitted JS shape against the wasm-rquickjs d.ts
      // quirks directly, so a consistently-wrong tag/field on both sides cannot hide.
      test("raw JS shape: variant-value uses `case_`, not `case`") {
        val node = singleValueNodeJs(VariantValue(WitVariantValuePayload(1, Some(3))))
        val pay  = valDict(node)
        assertTrue(
          node.tag == "variant-value",
          pay.contains("case_"),
          !pay.contains("case"),
          pay("case_").asInstanceOf[Int] == 1
        )
      },
      test("raw JS shape: quantity value node tag is `quantity-value-node`") {
        assertTrue(singleValueNodeJs(QuantityValueNode(qvalue)).tag == "quantity-value-node")
      },
      test("raw JS shape: result value tags are `ok-value` / `err-value`") {
        val ok  = valDict(singleValueNodeJs(ResultValue(WitResultValuePayload.OkValue(Some(9)))))
        val err = valDict(singleValueNodeJs(ResultValue(WitResultValuePayload.ErrValue(None))))
        assertTrue(ok("tag").asInstanceOf[String] == "ok-value", err("tag").asInstanceOf[String] == "err-value")
      },
      test("raw JS shape: path direction/kind are plain strings") {
        val spec = valDict(singleTypeBodyJs(PathType(pathFull)))
        assertTrue(
          spec("direction").asInstanceOf[String] == "in-out",
          spec("kind").asInstanceOf[String] == "directory"
        )
      },
      test("raw JS shape: binary payload bytes is a Uint8Array") {
        val pay =
          valDict(singleValueNodeJs(BinaryValue(WitBinaryValuePayload(Vector[Byte](1, 2, 3), Some("image/png")))))
        val ctorName = pay("bytes").asInstanceOf[js.Dynamic].constructor.name.asInstanceOf[String]
        val bytes    = pay("bytes").asInstanceOf[Uint8Array]
        assertTrue(ctorName == "Uint8Array", bytes.length == 3, bytes(0).toInt == 1, bytes(2).toInt == 3)
      },
      test("raw JS shape: s64/u64 carry JS bigint, u32 carries JS number") {
        val s64 = rawVal(singleValueNodeJs(S64Value(-64L)))
        val u64 = rawVal(singleValueNodeJs(U64Value(-1L)))
        val u32 = rawVal(singleValueNodeJs(U32Value(4294967295L)))
        assertTrue(
          js.typeOf(s64) == "bigint",
          js.typeOf(u64) == "bigint",
          js.typeOf(u32) == "number"
        )
      }
    )

  // --- raw-shape helpers -----------------------------------------------------

  /** Encode a single value node and return its `{ tag, val }` JS facade. */
  private def singleValueNodeJs(n: WitSchemaValueNode): JsSchemaValueNode =
    SchemaWireInterop.valueTreeToJs(WitSchemaValueTree(Vector(n), 0)).valueNodes(0)

  /** Encode a single type body and return its `{ tag, val }` JS facade. */
  private def singleTypeBodyJs(b: WitSchemaTypeBody): JsSchemaTypeBody =
    SchemaWireInterop.graphToJs(WitSchemaGraph(Vector(WitSchemaTypeNode(b, md0)), Vector.empty, 0)).typeNodes(0).body

  /** Read the raw `val` payload of a `{ tag, val }` object. */
  private def rawVal(o: js.Object): js.Any =
    o.asInstanceOf[js.Dynamic].selectDynamic("val")

  /**
   * Read the `val` payload of a `{ tag, val }` object as a string-keyed
   * dictionary.
   */
  private def valDict(o: js.Object): js.Dictionary[js.Any] =
    rawVal(o).asInstanceOf[js.Dictionary[js.Any]]
}
