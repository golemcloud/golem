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

import golem.schema.*
import golem.schema.SchemaTypeBody.*
import golem.schema.validation.WellFormedness
import zio.blocks.schema.json.Json
import zio.test.*

import scala.collection.immutable.ListMap
import scala.collection.mutable

object ReflectionConformanceSpec extends ZIOSpecDefault {
  private def fields(value: Json): ListMap[String, Json] = value match {
    case Json.Object(values) => ListMap(values.toList*)
    case other               => throw new AssertionError(s"expected JSON object, got $other")
  }

  private def elements(value: Json): List[Json] = value match {
    case Json.Array(values) => values.toList
    case other              => throw new AssertionError(s"expected JSON array, got $other")
  }

  private def string(value: Json): String = value match {
    case Json.String(value) => value
    case other              => throw new AssertionError(s"expected JSON string, got $other")
  }

  private def count(value: Json): Int = value match {
    case Json.Number(value) => value.toInt
    case other              => throw new AssertionError(s"expected JSON number, got $other")
  }

  private def boolean(value: Json): Boolean = value match {
    case Json.Boolean(value) => value
    case other               => throw new AssertionError(s"expected JSON boolean, got $other")
  }

  private def field(name: String, body: SchemaType): NamedFieldType = NamedFieldType(name, body)

  private def fixture(name: String): SchemaRef = {
    val root = name match {
      case "s64"             => SchemaType(S64Type())
      case "constrained-s64" =>
        SchemaType(
          S64Type(
            Some(
              NumericRestrictions(
                min = Some(NumericBound.Signed(-9007199254740993L)),
                max = Some(NumericBound.Signed(9007199254740993L))
              )
            )
          )
        )
      case "u64"        => SchemaType(U64Type())
      case "binary"     => SchemaType(BinaryType(BinaryRestrictions.empty))
      case "duration"   => SchemaType(DurationType)
      case "quantity"   => SchemaType(QuantityType(QuantitySpec("m", Nil, None, None)))
      case "tool-input" =>
        SchemaType(
          RecordType(
            List(
              field("pattern", SchemaType(StringType)),
              field("paths", SchemaType(ListType(SchemaType(StringType)))),
              field("ignoreCase", SchemaType(OptionType(SchemaType(BoolType))))
            )
          )
        )
      case "config-entry" =>
        SchemaType(
          RecordType(
            List(
              field("path", SchemaType(ListType(SchemaType(StringType)))),
              field("value", SchemaType(S64Type()))
            )
          )
        )
      case "constrained-u32" =>
        SchemaType(
          U32Type(
            Some(
              NumericRestrictions(
                min = Some(NumericBound.Unsigned(2)),
                max = Some(NumericBound.Unsigned(10))
              )
            )
          )
        )
      case "constrained-f64" =>
        SchemaType(
          F64Type(
            Some(
              NumericRestrictions(
                min = Some(NumericBound.FloatBits(java.lang.Double.doubleToRawLongBits(-1.5))),
                max = Some(NumericBound.FloatBits(java.lang.Double.doubleToRawLongBits(2.5)))
              )
            )
          )
        )
      case "constrained-text" =>
        SchemaType(
          TextType(
            TextRestrictions(
              languages = Some(List("en", "de")),
              minLength = Some(2),
              maxLength = Some(8),
              regex = Some("^[a-z]+$")
            )
          )
        )
      case "constrained-binary" =>
        SchemaType(
          BinaryType(
            BinaryRestrictions(
              mimeTypes = Some(List("image/png", "application/octet-stream")),
              minBytes = Some(2),
              maxBytes = Some(4)
            )
          )
        )
      case "result" =>
        SchemaType(ResultType(Some(SchemaType(StringType)), Some(SchemaType(U32Type()))))
      case "custom-error" =>
        SchemaType(
          ResultType(
            Some(SchemaType(StringType)),
            Some(
              SchemaType(
                RecordType(
                  List(
                    field("code", SchemaType(StringType)),
                    field("retryable", SchemaType(BoolType))
                  )
                )
              )
            )
          )
        )
      case "optional-record" =>
        val optional = SchemaType(OptionType(SchemaType(StringType)))
        return SchemaRef(
          SchemaGraph(
            ListMap("conformance.optional" -> SchemaTypeDef(optional)),
            SchemaType(
              RecordType(
                List(
                  field("direct", optional),
                  field("referenced", SchemaType(RefType("conformance.optional")))
                )
              )
            )
          )
        )
      case other => throw new AssertionError(s"unknown conformance fixture $other")
    }
    SchemaRef(SchemaGraph(ListMap.empty, root))
  }

  private def atPointer(value: Json, pointer: String): Json =
    if (pointer.isEmpty) value
    else
      pointer
        .stripPrefix("/")
        .split('/')
        .foldLeft(value) { (current, part) =>
          fields(current).getOrElse(
            part.replace("~1", "/").replace("~0", "~"),
            throw new AssertionError(s"missing JSON pointer $pointer in $value")
          )
        }

  private def assertSubset(actual: Json, expected: Json): Unit = expected match {
    case Json.Object(expectedFields) =>
      val actualFields = fields(actual)
      expectedFields.toList.foreach { case (name, value) =>
        assertSubset(
          actualFields.getOrElse(name, throw new AssertionError(s"missing $name in $actual")),
          value
        )
      }
    case _ => Predef.assert(actual == expected, s"expected $expected, got $actual")
  }

  private def assertSemantic(name: String, expected: Json, corpus: Json): Unit = name match {
    case "unsupported-leaves" =>
      val unsupported = List[SchemaTypeBody](
        SecretType(SecretSpec(SchemaType(StringType), None)),
        QuotaTokenType(QuotaTokenSpec(None)),
        PermissionCardType(PermissionCardSpec(polymorphic = false)),
        FutureType(None),
        StreamType(None)
      )
      Predef.assert(unsupported.size == count(fields(expected)("count")))
      unsupported.foreach { body =>
        assertSubset(
          SchemaRef(SchemaGraph(ListMap.empty, SchemaType(body))).toJsonSchema(false),
          fields(expected)("schema")
        )
      }
    case "all-kinds" =>
      val supported = List(
        "ref",
        "bool",
        "s8",
        "s16",
        "s32",
        "s64",
        "u8",
        "u16",
        "u32",
        "u64",
        "f32",
        "f64",
        "char",
        "string",
        "record",
        "variant",
        "enum",
        "flags",
        "tuple",
        "list",
        "fixed-list",
        "map",
        "option",
        "result",
        "text",
        "binary",
        "path",
        "url",
        "datetime",
        "duration",
        "quantity",
        "union",
        "secret",
        "quota-token",
        "permission-card",
        "future",
        "stream"
      )
      val expectedNames = elements(fields(expected)("names")).map(string)
      Predef.assert(supported == expectedNames)
      Predef.assert(elements(fields(corpus)("schemaKinds")).map(string) == expectedNames)
    case "all-restrictions" =>
      val supported = List(
        "numeric-minimum",
        "numeric-maximum",
        "numeric-unit",
        "text-languages",
        "text-min-length",
        "text-max-length",
        "text-regex",
        "binary-mime-types",
        "binary-min-bytes",
        "binary-max-bytes",
        "path-direction",
        "path-kind",
        "path-mime-types",
        "path-extensions",
        "url-schemes",
        "url-hosts",
        "quantity-base-unit",
        "quantity-suffixes",
        "quantity-minimum",
        "quantity-maximum",
        "union-prefix",
        "union-suffix",
        "union-regex",
        "union-field"
      )
      val expectedNames = elements(fields(expected)("names")).map(string)
      Predef.assert(supported == expectedNames)
      Predef.assert(elements(fields(corpus)("restrictionKinds")).map(string) == expectedNames)
    case "graph" =>
      val expectedFields = fields(expected)
      val referenced     = fixture("optional-record")
      val inline         = SchemaRef(
        SchemaGraph(
          ListMap.empty,
          SchemaType(
            RecordType(
              List(
                field("direct", SchemaType(OptionType(SchemaType(StringType)))),
                field("referenced", SchemaType(OptionType(SchemaType(StringType))))
              )
            )
          )
        )
      )
      val valid      = WellFormedness.validateGraph(referenced.graph).isRight
      val equivalent = referenced.packJson(Json.Object()) == inline.packJson(Json.Object())
      Predef.assert(valid == boolean(expectedFields("valid")))
      Predef.assert(equivalent == boolean(expectedFields("equivalent")))
    case other => throw new AssertionError(s"unknown semantic conformance fixture $other")
  }

  override def spec = suite("reflection conformance corpus")(
    test("executes the complete declared case-ID set") {
      val corpus = Json.parse(ReflectionConformanceCorpus.json).fold(throw _, identity)
      val root   = fields(corpus)
      Predef.assert(string(root("version")) == "1.0.0")
      val executed = mutable.Set.empty[String]
      elements(root("cases")).foreach { testCaseJson =>
        val testCase = fields(testCaseJson)
        val id       = string(testCase("id"))
        Predef.assert(executed.add(id), s"duplicate case ID $id")
        val name = string(testCase("fixture"))
        string(testCase("operation")) match {
          case "roundtrip" =>
            val schema = fixture(name)
            val packed =
              schema.packJson(testCase("input")).fold(error => throw new AssertionError(s"$id: $error"), identity)
            Predef.assert(schema.unpackJson(packed) == Right(testCase("expected")), id)
          case "reject" =>
            val inputs = testCase.get("inputs").map(elements).getOrElse(List(testCase("input")))
            inputs.foreach { input =>
              val schema = fixture(name)
              val actual = schema.packJson(input) match {
                case Left(_)                                            => "invalid-json"
                case Right(value) if schema.validateValue(value).isLeft => "constraint-violation"
                case Right(value)                                       => throw new AssertionError(s"$id accepted $input as $value")
              }
              Predef.assert(actual == string(fields(testCase("expected"))("kind")), id)
            }
          case "json-schema" =>
            assertSubset(
              atPointer(fixture(name).toJsonSchema(false), string(testCase("path"))),
              testCase("expected")
            )
          case "semantic" => assertSemantic(name, testCase("expected"), corpus)
          case operation  => throw new AssertionError(s"unknown conformance operation $operation for $id")
        }
      }
      val declared = elements(root("caseIds")).map(string).toSet
      assertTrue(executed.toSet == declared)
    }
  )
}
