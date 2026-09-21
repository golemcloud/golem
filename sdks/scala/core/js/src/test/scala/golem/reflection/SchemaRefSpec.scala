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
                  SchemaType(TextType(TextRestrictions(minLength = Some(12), regex = Some("^https://"))))
                ),
                NamedFieldType(
                  "content",
                  SchemaType(BinaryType(BinaryRestrictions(minBytes = Some(3), maxBytes = Some(6))))
                )
              )
            )
          )
        )
      )
      val rendered   = restricted.toJsonSchema()
      val properties = rendered.get("properties").one.toOption.get
      val count      = properties.get("count").one.toOption.get
      val text       = properties.get("message").one.toOption.get.get("properties").one.toOption.get.get("text").one
      val bytes      = properties.get("content").one.toOption.get.get("properties").one.toOption.get.get("bytes").one
      assertTrue(
        count.get("maximum").one == Right(Json.Number(BigDecimal(3))),
        text.flatMap(_.get("minLength").one) == Right(Json.Number(BigDecimal(12))),
        text.flatMap(_.get("pattern").one) == Right(Json.String("^https://")),
        bytes.flatMap(_.get("minLength").one) == Right(Json.Number(BigDecimal(4))),
        bytes.flatMap(_.get("maxLength").one) == Right(Json.Number(BigDecimal(8)))
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
    test("requires explicit null for an absent option and renders it as required") {
      val optional = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(RecordType(List(NamedFieldType("maybe", SchemaType(OptionType(SchemaType(StringType)))))))
        )
      )
      assertTrue(
        optional.packJson(Json.Object()).isLeft,
        optional.packJson(Json.Object("maybe" -> Json.Null)) == Right(RecordValue(List(OptionValue(None)))),
        optional.toJsonSchema().get("required").one == Right(Json.Array(Json.String("maybe")))
      )
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
    test("caller-owned contracts expose two tiers and validate full identity shapes") {
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
      assertTrue(good.isRight, unknown.isLeft, secret.isLeft, invalid.isLeft, restricted.isLeft)
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
