/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.reflection

import golem.config.ConfigOverride
import golem.schema._
import golem.schema.SchemaTypeBody._
import golem.schema.SchemaValue._
import golem.runtime.InputRecordCodec
import zio.test._
import zio.blocks.schema.json.Json

import scala.collection.immutable.ListMap

object SchemaRefSpec extends ZIOSpecDefault {
  private val graph = SchemaGraph(
    ListMap.empty,
    SchemaType(
      RecordType(
        List(
          NamedFieldType("name", SchemaType(StringType)),
          NamedFieldType("count", SchemaType(U32Type())),
          NamedFieldType("enabled", SchemaType(BoolType)),
          NamedFieldType("labels", SchemaType(ListType(SchemaType(StringType))))
        )
      )
    )
  )

  def spec = suite("SchemaRef")(
    test("packs and unpacks canonical record JSON") {
      val ref  = SchemaRef(graph)
      val json = Json.Object(
        "name"    -> Json.String("worker"),
        "count"   -> Json.Number(BigDecimal(42)),
        "enabled" -> Json.Boolean(true),
        "labels"  -> Json.Array(Json.String("a"), Json.String("b"))
      )
      val expected = RecordValue(
        List(StringValue("worker"), U32Value(42), BoolValue(true), ListValue(List(StringValue("a"), StringValue("b"))))
      )
      assertTrue(ref.packJson(json) == Right(expected), ref.unpackJson(expected) == Right(json))
    },
    test("rejects unknown fields and invalid direct values") {
      val ref         = SchemaRef(graph)
      val invalidJson = Json.Object(
        "name"    -> Json.String("worker"),
        "count"   -> Json.Number(BigDecimal(1)),
        "enabled" -> Json.Boolean(true),
        "labels"  -> Json.Array(),
        "extra"   -> Json.String("no")
      )
      assertTrue(
        ref.packJson(invalidJson).isLeft,
        ref.validateValue(RecordValue(List(StringValue("too-short")))).isLeft
      )
    },
    test("round-trips rich canonical JSON values") {
      val rich = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(
            TupleType(
              List(
                SchemaType(BinaryType(BinaryRestrictions.empty)),
                SchemaType(DatetimeType),
                SchemaType(DurationType)
              )
            )
          )
        )
      )
      val value = TupleValue(
        List(
          BinaryValue(Vector(0, 1, 2, -1), Some("application/octet-stream")),
          DatetimeValue(Datetime(1704067200L, 123000000)),
          DurationValue(3723000000004L)
        )
      )
      assertTrue(rich.unpackJson(value).flatMap(rich.packJson) == Right(value))
    },
    test("does not expose capabilities as JSON") {
      val ref = SchemaRef(
        SchemaGraph(ListMap.empty, SchemaType(PermissionCardType(PermissionCardSpec(polymorphic = false))))
      )
      assertTrue(ref.packJson(Json.Null).isLeft)
    },
    test("renders canonical JSON Schema") {
      val rendered = SchemaRef(graph).toJsonSchema()
      assertTrue(
        rendered.get("$schema").one == Right(Json.String("https://json-schema.org/draft/2020-12/schema")),
        rendered.get("type").one == Right(Json.String("object"))
      )
    },
    test("validates and renders numeric, text, and binary restrictions") {
      val restricted = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(
            RecordType(
              List(
                NamedFieldType(
                  "count",
                  SchemaType(S32Type(Some(NumericRestrictions(max = Some(NumericBound.Signed(3))))))
                ),
                NamedFieldType(
                  "message",
                  SchemaType(
                    TextType(
                      TextRestrictions(
                        languages = Some(List("en", "de")),
                        minLength = Some(12),
                        regex = Some("^https://")
                      )
                    )
                  )
                ),
                NamedFieldType(
                  "content",
                  SchemaType(
                    BinaryType(
                      BinaryRestrictions(
                        mimeTypes = Some(List("image/png")),
                        minBytes = Some(3),
                        maxBytes = Some(6)
                      )
                    )
                  )
                )
              )
            )
          )
        )
      )
      val rendered         = restricted.toJsonSchema()
      val properties       = rendered.get("properties").one.toOption.get
      val count            = properties.get("count").one.toOption.get
      val textSchema       = properties.get("message").one.toOption.get
      val textProperties   = textSchema.get("properties").one.toOption.get
      val text             = textProperties.get("text").one
      val binarySchema     = properties.get("content").one.toOption.get
      val binaryProperties = binarySchema.get("properties").one.toOption.get
      val bytes            = binaryProperties.get("bytes").one
      val mimeType         = binaryProperties.get("mimeType").one
      val pattern          = bytes.flatMap(_.get("pattern").one) match {
        case Right(Json.String(value)) => value
        case other                     => throw new AssertionError(s"expected binary pattern, got $other")
      }
      val canonical    = List("", "AQ", "AQI", "AQID", "-_8").forall(value => pattern.r.pattern.matcher(value).matches())
      val nonCanonical = List("+/8", "AQ==", "-_9", "A").forall(value => !pattern.r.pattern.matcher(value).matches())
      assertTrue(
        count.get("maximum").one == Right(Json.Number(BigDecimal(3))),
        text.flatMap(_.get("minLength").one) == Right(Json.Number(BigDecimal(12))),
        text.flatMap(_.get("pattern").one) == Right(Json.String("^https://")),
        textProperties.get("language").one.flatMap(_.get("enum").one) ==
          Right(Json.Array(Json.String("en"), Json.String("de"))),
        textSchema.get("description").one.isLeft,
        bytes.flatMap(_.get("minLength").one) == Right(Json.Number(BigDecimal(4))),
        bytes.flatMap(_.get("maxLength").one) == Right(Json.Number(BigDecimal(8))),
        pattern == "^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-][AQgw]|[A-Za-z0-9_-]{2}[AEIMQUYcgkosw048])?$",
        canonical,
        nonCanonical,
        mimeType.flatMap(_.get("pattern").one) ==
          Right(Json.String("^[A-Za-z0-9!#$&^_.+\\-]+\\/[A-Za-z0-9!#$&^_.+\\-]+$")),
        mimeType.flatMap(_.get("enum").one) == Right(Json.Array(Json.String("image/png"))),
        binarySchema.get("description").one.isLeft
      )
    },
    test("union export keeps discriminator and branch body while packing enforces the rule") {
      val union = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(
            UnionType(
              List(
                UnionBranch("ssh", SchemaType(StringType), DiscriminatorRule.Prefix("ssh://")),
                UnionBranch("https", SchemaType(StringType), DiscriminatorRule.Regex("^https://"))
              )
            )
          )
        )
      )
      val branches = Json.Array(
        Json.Object(
          "allOf" -> Json.Array(
            Json.Object("type"    -> Json.String("string")),
            Json.Object("pattern" -> Json.String("^ssh://"))
          )
        ),
        Json.Object(
          "allOf" -> Json.Array(
            Json.Object("type"    -> Json.String("string")),
            Json.Object("pattern" -> Json.String("^https://"))
          )
        )
      )
      assertTrue(
        union.toJsonSchema().get("oneOf").one == Right(branches),
        union.packJson(Json.String("ssh://host")).isRight,
        union.packJson(Json.String("http://host")).isLeft
      )
    },
    test("decodes omitted and explicit-null option fields as absent") {
      val optional = SchemaRef(
        SchemaGraph(
          ListMap("maybe-ref" -> SchemaTypeDef(SchemaType(OptionType(SchemaType(StringType))))),
          SchemaType(
            RecordType(
              List(
                NamedFieldType("maybe", SchemaType(OptionType(SchemaType(StringType)))),
                NamedFieldType("referenced", SchemaType(RefType("maybe-ref")))
              )
            )
          )
        )
      )
      val absent        = RecordValue(List(OptionValue(None), OptionValue(None)))
      val malformedRefs = SchemaRef(
        SchemaGraph(
          ListMap(
            "cycle-a" -> SchemaTypeDef(SchemaType(RefType("cycle-b"))),
            "cycle-b" -> SchemaTypeDef(SchemaType(RefType("cycle-a")))
          ),
          SchemaType(
            RecordType(
              List(
                NamedFieldType("dangling", SchemaType(RefType("missing"))),
                NamedFieldType("cycle", SchemaType(RefType("cycle-a")))
              )
            )
          )
        )
      )
      assertTrue(
        optional.packJson(Json.Object()) == Right(absent),
        optional.packJson(Json.Object("maybe" -> Json.Null, "referenced" -> Json.Null)) == Right(absent),
        optional.toJsonSchema().get("required").one == Right(Json.Array()),
        malformedRefs.toJsonSchema().get("required").one ==
          Right(Json.Array(Json.String("dangling"), Json.String("cycle")))
      )
    },
    test("uses lossless canonical JSON for wide integers, durations, and quantities") {
      val wide = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(
            RecordType(
              List(
                NamedFieldType("signed", SchemaType(S64Type())),
                NamedFieldType("unsigned", SchemaType(U64Type())),
                NamedFieldType("duration", SchemaType(DurationType)),
                NamedFieldType(
                  "quantity",
                  SchemaType(QuantityType(QuantitySpec("m", Nil, None, None)))
                )
              )
            )
          )
        )
      )
      val json = Json.Object(
        "signed"   -> Json.String(Long.MinValue.toString),
        "unsigned" -> Json.String("18446744073709551615"),
        "duration" -> Json.Object("nanoseconds" -> Json.String(Long.MaxValue.toString)),
        "quantity" -> Json.Object(
          "mantissa" -> Json.String(Long.MinValue.toString),
          "scale"    -> Json.Number(BigDecimal(-2)),
          "unit"     -> Json.String("m")
        )
      )
      val value = RecordValue(
        List(
          S64Value(Long.MinValue),
          U64Value(-1L),
          DurationValue(Long.MaxValue),
          QuantityValueNode(QuantityValue(Long.MinValue, -2, "m"))
        )
      )
      val rendered = wide.toJsonSchema(includeDraftMarker = false)
      val props    = rendered.get("properties").one.toOption.get
      assertTrue(
        wide.packJson(json) == Right(value),
        wide.unpackJson(value) == Right(json),
        props.get("signed").one.flatMap(_.get("pattern").one) ==
          Right(Json.String("^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$")),
        props.get("unsigned").one.flatMap(_.get("x-golem-maximum").one) ==
          Right(Json.String("18446744073709551615")),
        props.get("duration").one.flatMap(_.get("type").one) == Right(Json.String("object")),
        props
          .get("quantity")
          .one
          .flatMap(_.get("properties").one)
          .flatMap(_.get("mantissa").one)
          .flatMap(_.get("type").one) == Right(Json.String("string"))
      )
    },
    test("rejects non-canonical or overflowing wide decimal strings") {
      val signed   = SchemaRef(SchemaGraph(ListMap.empty, SchemaType(S64Type())))
      val unsigned = SchemaRef(SchemaGraph(ListMap.empty, SchemaType(U64Type())))
      val duration = SchemaRef(SchemaGraph(ListMap.empty, SchemaType(DurationType)))
      val quantity = SchemaRef(
        SchemaGraph(ListMap.empty, SchemaType(QuantityType(QuantitySpec("m", Nil, None, None))))
      )
      assertTrue(
        List(Json.String("+1"), Json.String("01"), Json.String("-0"), Json.String("9223372036854775808"))
          .forall(signed.packJson(_).isLeft),
        List(Json.String("-1"), Json.String("+1"), Json.String("01"), Json.String("18446744073709551616"))
          .forall(unsigned.packJson(_).isLeft),
        duration.packJson(Json.String("PT1S")).isLeft,
        duration.packJson(Json.Object("nanoseconds" -> Json.Number(BigDecimal(1)))).isLeft,
        quantity
          .packJson(
            Json.Object(
              "mantissa" -> Json.String("-0"),
              "scale"    -> Json.Number(BigDecimal(0)),
              "unit"     -> Json.String("m")
            )
          )
          .isLeft
      )
    },
    test("reflection JSON Schema rejects leaves with no JSON representation") {
      val leaves = List[SchemaTypeBody](
        SecretType(SecretSpec(SchemaType(StringType), None)),
        QuotaTokenType(QuotaTokenSpec(None)),
        PermissionCardType(PermissionCardSpec(polymorphic = false)),
        FutureType(None),
        StreamType(None)
      )
      assertTrue(leaves.forall { body =>
        SchemaRef(SchemaGraph(ListMap.empty, SchemaType(body)))
          .toJsonSchema(includeDraftMarker = false)
          .get("not")
          .one == Right(Json.Object())
      })
    },
    test("rejects numbers that overflow after float narrowing") {
      val f32 = SchemaRef(SchemaGraph(ListMap.empty, SchemaType(F32Type())))
      val f64 = SchemaRef(SchemaGraph(ListMap.empty, SchemaType(F64Type())))
      assertTrue(
        f32.packJson(Json.Number(BigDecimal("1e100"))).isLeft,
        f64.packJson(Json.Number(BigDecimal("1e1000"))).isLeft
      )
    },
    test("detects nested stream schemas") {
      val streaming = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(RecordType(List(NamedFieldType("items", SchemaType(StreamType(Some(SchemaType(StringType))))))))
        )
      )
      val selectedScalar = SchemaRef(streaming.graph, SchemaType(StringType))
      assertTrue(!SchemaRef(graph).containsStream, streaming.containsStream, !selectedScalar.containsStream)
    },
    test("rejects missing, unexpected, and malformed reflected outputs") {
      val input    = SchemaRef(SchemaGraph(ListMap.empty, SchemaType(RecordType(Nil))))
      val output   = SchemaRef(SchemaGraph(ListMap.empty, SchemaType(StringType)))
      val metadata = InvocationMetadata(ParsedAgentId("test"), "key")
      val unit     = AgentMethod("unit", "", None, input, None)
      val single   = AgentMethod("single", "", None, input, Some(output))

      assertTrue(
        ReflectionInternals.validateInvocationOutput(unit, Invocation(metadata, None)).isRight,
        ReflectionInternals.validateInvocationOutput(unit, Invocation(metadata, Some(StringValue("extra")))).isLeft,
        ReflectionInternals.validateInvocationOutput(single, Invocation(metadata, None)).isLeft,
        ReflectionInternals.validateInvocationOutput(single, Invocation(metadata, Some(U32Value(1)))).isLeft,
        ReflectionInternals
          .validateInvocationOutput(single, Invocation(metadata, Some(StringValue("ok"))))
          .isRight
      )
    },
    test("caller-defined static clients expose method-only and full options and validate full identity shapes") {
      val binding: AgentClientDefinition[MethodOnly, Unit, NoConfig] = AgentClientDefinition.methodOnly
      val full: AgentClientDefinition[DurableFull, String, NoConfig] = AgentClientDefinition.full(
        name = "CounterAgent",
        constructor = InputRecordCodec.single[String]("name")
      )
      val wrongShape = ReflectionInternals.validate(
        SchemaRef(full.constructorCodec.get.graph),
        RecordValue(List(U32Value(1)))
      )
      val configured = AgentClientDefinition.full[String, String](
        name = "ConfiguredCounterAgent",
        mode = AgentMode.Durable,
        constructor = InputRecordCodec.single[String]("name"),
        config = AgentConfigCodec[String](_ => Nil)
      )
      val optionalConfig: String => Either[GolemReflectError, CallerCodecAgentClient] = configured.client.get

      assertTrue(
        binding.contractName.isEmpty,
        full.contractName.contains("CounterAgent"),
        wrongShape.isLeft,
        optionalConfig != null
      )
    },
    test("reflected config validates declared local paths before RPC creation") {
      val stringSchema = SchemaRef(SchemaGraph(ListMap.empty, SchemaType(StringType)))
      val countSchema  = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(S32Type(Some(NumericRestrictions(max = Some(NumericBound.Signed(3))))))
        )
      )
      val agentType = new AgentType(
        "ConfiguredCounterAgent",
        "",
        "scala",
        AgentMode.Durable,
        ComponentId(golem.Uuid(BigInt(0), BigInt(1))),
        SchemaRef(graph),
        Nil,
        List(
          ReflectedConfigDeclaration(List("greeting"), "local", stringSchema),
          ReflectedConfigDeclaration(List("count"), "local", countSchema),
          ReflectedConfigDeclaration(List("apiKey"), "secret", stringSchema)
        )
      )
      val good       = agentType.packConfigJson(List(ReflectedConfigJson(List("greeting"), Json.String("hello"))))
      val unknown    = agentType.packConfigJson(List(ReflectedConfigJson(List("unknown"), Json.String("x"))))
      val secret     = agentType.packConfigJson(List(ReflectedConfigJson(List("apiKey"), Json.String("x"))))
      val invalid    = agentType.packConfigJson(List(ReflectedConfigJson(List("greeting"), Json.Number(BigDecimal(42)))))
      val restricted = agentType.packConfigJson(List(ReflectedConfigJson(List("count"), Json.Number(BigDecimal(4)))))
      val mismatched = agentType.validateConfig(
        List(
          ConfigOverride(
            List("greeting"),
            TypedSchemaValue(SchemaGraph(ListMap.empty, SchemaType(U32Type())), StringValue("hello"))
          )
        )
      )
      val equivalent = agentType.validateConfig(
        List(
          ConfigOverride(
            List("greeting"),
            TypedSchemaValue(
              SchemaGraph(ListMap("alias" -> SchemaTypeDef(SchemaType(StringType))), SchemaType(RefType("alias"))),
              StringValue("hello")
            )
          )
        )
      )
      assertTrue(
        good.isRight,
        unknown.isLeft,
        secret.isLeft,
        invalid.isLeft,
        restricted.isLeft,
        mismatched.isLeft,
        equivalent.isRight
      )
    },
    test("binary JSON Schema bounds use wider arithmetic") {
      val ref = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(BinaryType(BinaryRestrictions(minBytes = Some(Int.MaxValue), maxBytes = Some(Int.MaxValue))))
        )
      )
      val bytes = ref
        .toJsonSchema()
        .get("properties")
        .one
        .toOption
        .get
        .get("bytes")
        .one
        .toOption
        .get
      val expected = Json.Number(BigDecimal((Int.MaxValue.toLong * 4 + 2) / 3))
      assertTrue(bytes.get("minLength").one == Right(expected), bytes.get("maxLength").one == Right(expected))
    },
    test("binary MIME syntax is canonical while MIME metadata stays optional") {
      val ref = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(BinaryType(BinaryRestrictions(mimeTypes = Some(List("image/png")))))
        )
      )
      val withoutMime = Json.Object("bytes" -> Json.String("AQ"))
      val invalidMime = Json.Object("bytes" -> Json.String("AQ"), "mimeType" -> Json.String("not a mime"))
      assertTrue(
        ref.validateJson(withoutMime).isRight,
        ref.packJson(invalidMime).isLeft,
        ref.unpackJson(BinaryValue(Vector[Byte](1), Some("not a mime"))).isLeft
      )
    },
    test("throwing config codecs return schema encode failures") {
      val definition = AgentClientDefinition.full[String, String](
        name = "ConfiguredCounterAgent",
        mode = AgentMode.Durable,
        constructor = InputRecordCodec.single[String]("name"),
        config = AgentConfigCodec[String](_ => throw new IllegalArgumentException("bad config"))
      )
      val client   = definition.client
      val expected = Left(GolemReflectError.SchemaEncode("bad config"))
      val phantom  = golem.Uuid(BigInt(0), BigInt(1))
      assertTrue(
        client.get("worker", "bad") == expected,
        client.getPhantom("worker", phantom, "bad") == expected,
        client.newPhantom("worker", "bad") == expected
      )
    }
  )
}
