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

import golem.schema.wire.*
import zio.blocks.schema.Schema
import zio.test.*

import scala.collection.immutable.ListMap
import scala.util.Try

object UnstructuredSpec extends ZIOSpecDefault {
  type Lang  = "en" | "de"
  type Image = "image/png" | "image/jpeg"

  final case class Document(title: String, body: UnstructuredText[Lang], attachments: List[UnstructuredBinary[String]])
  object Document { implicit val schema: Schema[Document] = Schema.derived }

  private val urlType = SchemaType(SchemaTypeBody.UrlType(UrlRestrictions.empty))

  private def expectedText(languages: Option[List[String]]): SchemaType =
    SchemaType(
      SchemaTypeBody.VariantType(
        List(
          VariantCaseType("inline", Some(SchemaType(SchemaTypeBody.TextType(TextRestrictions(languages = languages))))),
          VariantCaseType("url", Some(urlType))
        )
      ),
      MetadataEnvelope(role = Some(Role.UnstructuredText))
    )

  private def expectedBinary(mimeTypes: Option[List[String]]): SchemaType =
    SchemaType(
      SchemaTypeBody.VariantType(
        List(
          VariantCaseType(
            "inline",
            Some(SchemaType(SchemaTypeBody.BinaryType(BinaryRestrictions(mimeTypes = mimeTypes))))
          ),
          VariantCaseType("url", Some(urlType))
        )
      ),
      MetadataEnvelope(role = Some(Role.UnstructuredBinary))
    )

  private def graphOf(root: SchemaType): SchemaGraph = SchemaGraph(ListMap.empty, root)

  /**
   * Encodes through the concrete wire codec and the IntoSchema path and checks
   * they agree.
   */
  private def wireAgrees[A](value: A)(using codec: ConcreteCodec[A], into: IntoSchema[A], from: FromSchema[A]) = {
    val encoded = codec.encode(value)
    assertTrue(
      SchemaWire.schemaGraphFromWit(encoded.graph) == into.graph,
      SchemaWire.schemaValueFromWit(encoded.value) == into.toValue(value),
      codec.decode(encoded.value) == value,
      from.fromValue(into.toValue(value)) == Right(value)
    )
  }

  private given textCodec: ConcreteCodec[UnstructuredText[Lang]]                               = ConcreteCodec.derived
  private given binaryCodec: ConcreteCodec[UnstructuredBinary[Image]]                          = ConcreteCodec.derived
  private given nestedCodec: ConcreteCodec[Either[String, Option[UnstructuredBinary[String]]]] =
    ConcreteCodec.derived

  override def spec = suite("UnstructuredSpec")(
    test("unrestricted schemas are role-marked inline/url variants") {
      assertTrue(
        IntoSchema[UnstructuredText[String]].graph == graphOf(expectedText(None)),
        IntoSchema[UnstructuredBinary[String]].graph == graphOf(expectedBinary(None))
      )
    },
    test("literal union type parameters become the restriction allow-lists") {
      assertTrue(
        IntoSchema[UnstructuredText[Lang]].graph == graphOf(expectedText(Some(List("en", "de")))),
        IntoSchema[UnstructuredBinary[Image]].graph == graphOf(expectedBinary(Some(List("image/png", "image/jpeg")))),
        IntoSchema[UnstructuredBinary["application/pdf"]].graph ==
          graphOf(expectedBinary(Some(List("application/pdf"))))
      )
    },
    test("inline and url values use case 0 and case 1") {
      val text   = IntoSchema[UnstructuredText[Lang]]
      val binary = IntoSchema[UnstructuredBinary[Image]]
      assertTrue(
        text.toValue(UnstructuredText.fromInline("hallo", "de")) ==
          SchemaValue.VariantValue(0, Some(SchemaValue.TextValue("hallo", Some("de")))),
        text.toValue(UnstructuredText.fromUrl("https://example.com/a.txt")) ==
          SchemaValue.VariantValue(1, Some(SchemaValue.UrlValue("https://example.com/a.txt"))),
        binary.toValue(UnstructuredBinary.fromInline(Array[Byte](1, 2), "image/png")) ==
          SchemaValue.VariantValue(0, Some(SchemaValue.BinaryValue(Vector[Byte](1, 2), Some("image/png")))),
        binary.toValue(UnstructuredBinary.fromUrl("https://example.com/a.png")) ==
          SchemaValue.VariantValue(1, Some(SchemaValue.UrlValue("https://example.com/a.png")))
      )
    },
    test("text values round-trip through the wire and schema codecs") {
      wireAgrees[UnstructuredText[Lang]](UnstructuredText.fromInline("hello", "en")) &&
      wireAgrees[UnstructuredText[Lang]](UnstructuredText.fromInline("no language")) &&
      wireAgrees[UnstructuredText[Lang]](UnstructuredText.fromUrl("https://example.com/doc.txt"))
    },
    test("binary values round-trip through the wire and schema codecs") {
      wireAgrees[UnstructuredBinary[Image]](UnstructuredBinary.fromInline(Array[Byte](0, -1, 127), "image/jpeg")) &&
      wireAgrees[UnstructuredBinary[Image]](UnstructuredBinary.fromInline(Array.emptyByteArray)) &&
      wireAgrees[UnstructuredBinary[Image]](UnstructuredBinary.fromUrl("https://example.com/image.png"))
    },
    test("bare text and binary values decode as inline values") {
      val bareText   = WitSchemaValueTree(Vector(WitSchemaValueNode.TextValue(WitTextValuePayload("hi", Some("en")))), 0)
      val bareBinary = WitSchemaValueTree(
        Vector(WitSchemaValueNode.BinaryValue(WitBinaryValuePayload(Vector[Byte](7, 8), Some("image/png")))),
        0
      )
      assertTrue(
        textCodec.decode(bareText) == UnstructuredText.Inline[Lang]("hi", Some("en")),
        binaryCodec.decode(bareBinary) == UnstructuredBinary.Inline[Image](Array[Byte](7, 8), Some("image/png")),
        FromSchema[UnstructuredText[Lang]].fromValue(SchemaValue.TextValue("hi", None)) ==
          Right(UnstructuredText.Inline[Lang]("hi", None)),
        FromSchema[UnstructuredBinary[String]].fromValue(SchemaValue.BinaryValue(Vector[Byte](3), Some("x/y"))) ==
          Right(UnstructuredBinary.Inline[String](Array[Byte](3), Some("x/y")))
      )
    },
    test("values outside the allow-list are rejected") {
      val badText   = WitSchemaValueTree(Vector(WitSchemaValueNode.TextValue(WitTextValuePayload("hi", Some("fr")))), 0)
      val badBinary = WitSchemaValueTree(
        Vector(
          WitSchemaValueNode.BinaryValue(WitBinaryValuePayload(Vector[Byte](1), Some("text/plain"))),
          WitSchemaValueNode.VariantValue(WitVariantValuePayload(0, Some(0)))
        ),
        1
      )
      val wrongCase = WitSchemaValueTree(
        Vector(
          WitSchemaValueNode.UrlValue("https://example.com"),
          WitSchemaValueNode.VariantValue(WitVariantValuePayload(0, Some(0)))
        ),
        1
      )
      assertTrue(
        Try(textCodec.decode(badText)).isFailure,
        Try(binaryCodec.decode(badBinary)).isFailure,
        Try(binaryCodec.decode(wrongCase)).isFailure,
        FromSchema[UnstructuredText[Lang]].fromValue(SchemaValue.TextValue("hi", Some("fr"))).isLeft,
        FromSchema[UnstructuredBinary[Image]]
          .fromValue(SchemaValue.VariantValue(0, Some(SchemaValue.BinaryValue(Vector.empty, Some("a/b")))))
          .isLeft
      )
    },
    test("Option and Either nest the unstructured variant as result<option<_>, _>") {
      val expected = graphOf(
        t.result(Some(t.option(expectedBinary(None))), Some(t.string))
      )
      val values: List[Either[String, Option[UnstructuredBinary[String]]]] = List(
        Right(Some(UnstructuredBinary.fromInline(Array[Byte](1, 2, 3), "application/octet-stream"))),
        Right(Some(UnstructuredBinary.fromUrl("https://example.com/blob"))),
        Right(None),
        Left("failed")
      )
      assertTrue(
        IntoSchema[Either[String, Option[UnstructuredBinary[String]]]].graph == expected,
        IntoSchema[Option[UnstructuredBinary[Image]]].graph ==
          graphOf(t.option(expectedBinary(Some(List("image/png", "image/jpeg"))))),
        IntoSchema[Either[String, UnstructuredText[String]]].graph ==
          graphOf(t.result(Some(expectedText(None)), Some(t.string)))
      ) && values.map(wireAgrees(_)).reduce(_ && _)
    },
    test("nested bare values decode through the structural derivation") {
      val decoded = FromSchema[Either[String, Option[UnstructuredBinary[String]]]].fromValue(
        SchemaValue.ResultValue(
          SchemaResult.Ok(Some(SchemaValue.OptionValue(Some(SchemaValue.BinaryValue(Vector[Byte](9), None)))))
        )
      )
      assertTrue(decoded == Right(Right(Some(UnstructuredBinary.Inline[String](Array[Byte](9), None)))))
    },
    test("unstructured fields work inside derived records") {
      val value = Document(
        "doc",
        UnstructuredText.fromInline("text", "de"),
        List(UnstructuredBinary.fromUrl("https://example.com/x"), UnstructuredBinary.fromInline(Array[Byte](4)))
      )
      val into = IntoSchema[Document]
      val root = into.graph.defs.values.head.body.body
      assertTrue(
        root == SchemaTypeBody.RecordType(
          List(
            NamedFieldType("title", t.string),
            NamedFieldType("body", expectedText(Some(List("en", "de")))),
            NamedFieldType("attachments", t.list(expectedBinary(None)))
          )
        ),
        FromSchema[Document].fromValue(into.toValue(value)) == Right(value)
      )
    },
    test("only String or string literal types are accepted as allow-lists") {
      val errors: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        golem.schema.IntoSchema[golem.schema.UnstructuredText[Int]]
      """)
      val outsideAllowList: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        val image: golem.schema.UnstructuredBinary["image/png"] =
          golem.schema.UnstructuredBinary.fromInline(Array.emptyByteArray, "text/plain")
      """)
      assertTrue(errors.exists(_.message.contains("not a valid allow-list")), outsideAllowList.nonEmpty)
    }
  )
}
