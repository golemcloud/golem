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

  final case class Attachment[M](file: UnstructuredBinary[M])
  final case class Caption[L](text: UnstructuredText[L])
  final case class Gallery(
    png: Attachment["image/png"],
    jpeg: Attachment["image/jpeg"],
    either: Attachment["image/png" | "image/jpeg"],
    reversed: Attachment["image/jpeg" | "image/png"],
    aliased: Attachment[Image],
    any: Attachment[String],
    english: Caption["en"],
    other: Caption["de" | "fr"]
  )
  object Gallery { implicit val schema: Schema[Gallery] = Schema.derived }
  final case class PngJpeg(png: Attachment["image/png"], jpeg: Attachment["image/jpeg"])
  final case class LooseGallery(png: Attachment[String], jpeg: Attachment[String])

  object A { type Language = "en" }
  object B { type Language = "de" }
  type AOrFrench = A.Language | "fr"
  type BOrFrench = B.Language | "fr"
  type ALanguage = A.Language
  final case class Bilingual(
    first: Caption[A.Language | "fr"],
    second: Caption[B.Language | "fr"],
    nestedFirst: Caption[AOrFrench | "it"],
    nestedSecond: Caption[BOrFrench | "it"],
    aliasFirst: Caption[AOrFrench],
    aliasSecond: Caption[BOrFrench],
    chained: Caption[ALanguage | "fr"]
  )
  object Bilingual { implicit val schema: Schema[Bilingual] = Schema.derived }

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

  /**
   * Bypasses the allow-list typing to build a value a caller could still forge
   * at runtime.
   */
  private def forged[A](value: Any): A = value.asInstanceOf[A]

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
  private given galleryCodec: ConcreteCodec[Gallery]                                           = ConcreteCodec.derived
  private given bilingualCodec: ConcreteCodec[Bilingual]                                       = ConcreteCodec.derived
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
    test("bare text and binary values are rejected where the schema states the variant") {
      val bareText   = WitSchemaValueTree(Vector(WitSchemaValueNode.TextValue(WitTextValuePayload("hi", Some("en")))), 0)
      val bareBinary = WitSchemaValueTree(
        Vector(WitSchemaValueNode.BinaryValue(WitBinaryValuePayload(Vector[Byte](7, 8), Some("image/png")))),
        0
      )
      assertTrue(
        Try(textCodec.decode(bareText)).isFailure,
        Try(binaryCodec.decode(bareBinary)).isFailure,
        FromSchema[UnstructuredText[Lang]].fromValue(SchemaValue.TextValue("hi", None)).isLeft,
        FromSchema[UnstructuredBinary[String]].fromValue(SchemaValue.BinaryValue(Vector[Byte](3), Some("x/y"))).isLeft
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
    test("nested values decode through the structural derivation only in the variant form") {
      def nested(value: SchemaValue) =
        FromSchema[Either[String, Option[UnstructuredBinary[String]]]].fromValue(
          SchemaValue.ResultValue(SchemaResult.Ok(Some(SchemaValue.OptionValue(Some(value)))))
        )
      val bare = SchemaValue.BinaryValue(Vector[Byte](9), None)
      assertTrue(
        nested(SchemaValue.VariantValue(0, Some(bare))) ==
          Right(Right(Some(UnstructuredBinary.Inline[String](Array[Byte](9), None)))),
        nested(bare).isLeft
      )
    },
    test("an empty allow-list is unrestricted") {
      assertTrue(
        Unstructured.disallowed(Some("x/y"), Some(Nil), "MIME type").isEmpty,
        Unstructured.disallowed(Some("x/y"), None, "MIME type").isEmpty,
        Unstructured.disallowed(None, Some(List("a/b")), "MIME type").isEmpty,
        Unstructured.disallowed(Some("x/y"), Some(List("a/b")), "MIME type").nonEmpty,
        Unstructured.binaryFromSchemaValue(
          SchemaValue.VariantValue(0, Some(SchemaValue.BinaryValue(Vector.empty, Some("x/y")))),
          Some(Nil)
        ) == Right(UnstructuredBinary.Inline[String](Array.emptyByteArray, Some("x/y"))),
        Unstructured.textFromSchemaValue(
          SchemaValue.VariantValue(0, Some(SchemaValue.TextValue("hi", Some("fr")))),
          Some(Nil)
        ) == Right(UnstructuredText.Inline[String]("hi", Some("fr")))
      )
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
    test("distinct binary allow-lists in one derived codec keep their own restrictions") {
      type Pair = (UnstructuredBinary["image/png"], UnstructuredBinary["image/jpeg"])
      val codec        = ConcreteCodec.derived[Pair]
      val unrestricted = ConcreteCodec.derived[(UnstructuredBinary[String], UnstructuredBinary[String])]
      val value: Pair  = (
        UnstructuredBinary.fromInline(Array[Byte](1), "image/png"),
        UnstructuredBinary.fromInline(Array[Byte](2), "image/jpeg")
      )
      val pngTwice = unrestricted.encode(
        (
          UnstructuredBinary.fromInline(Array[Byte](1), "image/png"),
          UnstructuredBinary.fromInline(Array[Byte](2), "image/png")
        )
      )
      val encoded = Try(codec.encode(value))
      assertTrue(
        encoded
          .map(e => SchemaWire.schemaGraphFromWit(e.graph))
          .toOption
          .contains(
            graphOf(t.tuple(List(expectedBinary(Some(List("image/png"))), expectedBinary(Some(List("image/jpeg"))))))
          ),
        encoded.map(e => codec.decode(e.value)).toOption.contains(value),
        Try(codec.decode(pngTwice.value)).isFailure,
        Try(
          codec.encode(
            value.copy(_2 =
              forged[UnstructuredBinary["image/jpeg"]](UnstructuredBinary.fromInline(Array[Byte](2), "image/png"))
            )
          )
        ).isFailure
      )
    },
    test("distinct text allow-lists in one derived codec keep their own restrictions") {
      type Texts =
        (UnstructuredText["en"], UnstructuredText["de"], UnstructuredText["fr" | "it"], UnstructuredText["es" | "pt"])
      val codec        = ConcreteCodec.derived[Texts]
      val unrestricted = ConcreteCodec.derived[
        (UnstructuredText[String], UnstructuredText[String], UnstructuredText[String], UnstructuredText[String])
      ]
      val value: Texts = (
        UnstructuredText.fromInline("hello", "en"),
        UnstructuredText.fromInline("hallo", "de"),
        UnstructuredText.fromInline("ciao", "it"),
        UnstructuredText.fromInline("olá", "pt")
      )
      val leading = unrestricted.encode(
        (
          UnstructuredText.fromInline("hello", "en"),
          UnstructuredText.fromInline("hi", "en"),
          UnstructuredText.fromInline("salut", "fr"),
          UnstructuredText.fromInline("bonjour", "fr")
        )
      )
      val encoded = Try(codec.encode(value))
      assertTrue(
        encoded
          .map(e => SchemaWire.schemaGraphFromWit(e.graph))
          .toOption
          .contains(
            graphOf(
              t.tuple(
                List(
                  expectedText(Some(List("en"))),
                  expectedText(Some(List("de"))),
                  expectedText(Some(List("fr", "it"))),
                  expectedText(Some(List("es", "pt")))
                )
              )
            )
          ),
        encoded.map(e => codec.decode(e.value)).toOption.contains(value),
        Try(codec.decode(leading.value)).isFailure,
        Try(
          codec.encode(value.copy(_2 = forged[UnstructuredText["de"]](UnstructuredText.fromInline("hi", "en"))))
        ).isFailure,
        Try(
          codec.encode(
            value.copy(_4 = forged[UnstructuredText["es" | "pt"]](UnstructuredText.fromInline("salut", "fr")))
          )
        ).isFailure
      )
    },
    test("generic containers with distinct allow-lists are distinct named types") {
      val value = Gallery(
        Attachment(UnstructuredBinary.fromInline(Array[Byte](1), "image/png")),
        Attachment(UnstructuredBinary.fromInline(Array[Byte](2), "image/jpeg")),
        Attachment(UnstructuredBinary.fromInline(Array[Byte](3), "image/jpeg")),
        Attachment(UnstructuredBinary.fromInline(Array[Byte](4), "image/png")),
        Attachment(UnstructuredBinary.fromUrl("https://example.com/a.png")),
        Attachment(UnstructuredBinary.fromInline(Array[Byte](5), "text/plain")),
        Caption(UnstructuredText.fromInline("hello", "en")),
        Caption(UnstructuredText.fromInline("salut", "fr"))
      )
      val prefix       = "golem.schema.UnstructuredSpec."
      val binary       = (types: Option[List[String]]) => t.record(List(t.field("file", expectedBinary(types))))
      val text         = (langs: Option[List[String]]) => t.record(List(t.field("text", expectedText(langs))))
      val expectedDefs = Map(
        s"${prefix}Attachment<\"image/png\">"                  -> binary(Some(List("image/png"))),
        s"${prefix}Attachment<\"image/jpeg\">"                 -> binary(Some(List("image/jpeg"))),
        s"${prefix}Attachment<\"image/png\" | \"image/jpeg\">" -> binary(Some(List("image/png", "image/jpeg"))),
        s"${prefix}Attachment<\"image/jpeg\" | \"image/png\">" -> binary(Some(List("image/jpeg", "image/png"))),
        s"${prefix}Attachment<${prefix}Image>"                 -> binary(Some(List("image/png", "image/jpeg"))),
        s"${prefix}Attachment<java.lang.String>"               -> binary(None),
        s"${prefix}Caption<\"en\">"                            -> text(Some(List("en"))),
        s"${prefix}Caption<\"de\" | \"fr\">"                   -> text(Some(List("de", "fr")))
      )
      val concrete                   = Try(galleryCodec.encode(value)).map(e => SchemaWire.schemaGraphFromWit(e.graph))
      val runtime                    = IntoSchema[Gallery].graph
      def bodies(graph: SchemaGraph) = expectedDefs.keys.toList.map(id => id -> graph.defs.get(id).map(_.body)).toMap
      val jpegAsPng                  = value.copy(jpeg =
        forged[Attachment["image/jpeg"]](Attachment(UnstructuredBinary.fromInline(Array[Byte](2), "image/png")))
      )
      val loose = ConcreteCodec
        .derived[LooseGallery]
        .encode(
          LooseGallery(
            Attachment(UnstructuredBinary.fromInline(Array[Byte](1), "image/png")),
            Attachment(UnstructuredBinary.fromInline(Array[Byte](2), "image/png"))
          )
        )
      assertTrue(
        bodies(runtime) == expectedDefs.view.mapValues(Some(_)).toMap,
        concrete.map(bodies).toOption.contains(expectedDefs.view.mapValues(Some(_)).toMap),
        Try(galleryCodec.encode(jpegAsPng)).isFailure,
        Try(IntoSchema[Gallery].toValue(jpegAsPng)).isFailure,
        Try(ConcreteCodec.derived[PngJpeg].decode(loose.value)).isFailure
      ) && wireAgrees(value)
    },
    test("same-named literal aliases from different owners keep their own allow-lists") {
      type Pair = (UnstructuredText[A.Language | "fr"], UnstructuredText[B.Language | "fr"])
      val codec        = ConcreteCodec.derived[Pair]
      val unrestricted = ConcreteCodec.derived[(UnstructuredText[String], UnstructuredText[String])]
      val value: Pair  = (UnstructuredText.fromInline("hello", "en"), UnstructuredText.fromInline("hallo", "de"))
      val englishTwice =
        unrestricted.encode((UnstructuredText.fromInline("hello", "en"), UnstructuredText.fromInline("hi", "en")))
      val encoded = Try(codec.encode(value))
      assertTrue(
        encoded
          .map(e => SchemaWire.schemaGraphFromWit(e.graph))
          .toOption
          .contains(graphOf(t.tuple(List(expectedText(Some(List("en", "fr"))), expectedText(Some(List("de", "fr"))))))),
        encoded.map(e => codec.decode(e.value)).toOption.contains(value),
        Try(codec.decode(englishTwice.value)).isFailure,
        Try(
          codec.encode(
            value.copy(_2 = forged[UnstructuredText[B.Language | "fr"]](UnstructuredText.fromInline("hi", "en")))
          )
        ).isFailure
      )
    },
    test("generic containers over same-named literal aliases are distinct named types") {
      val value = Bilingual(
        Caption(UnstructuredText.fromInline("hello", "en")),
        Caption(UnstructuredText.fromInline("hallo", "de")),
        Caption(UnstructuredText.fromInline("ciao", "it")),
        Caption(UnstructuredText.fromInline("hallo", "de")),
        Caption(UnstructuredText.fromInline("salut", "fr")),
        Caption(UnstructuredText.fromInline("hallo", "de")),
        Caption(UnstructuredText.fromInline("hello", "en"))
      )
      val prefix       = "golem.schema.UnstructuredSpec."
      val text         = (langs: List[String]) => t.record(List(t.field("text", expectedText(Some(langs)))))
      val expectedDefs = Map(
        s"${prefix}Caption<${prefix}A.Language | \"fr\">" -> text(List("en", "fr")),
        s"${prefix}Caption<${prefix}B.Language | \"fr\">" -> text(List("de", "fr")),
        s"${prefix}Caption<${prefix}AOrFrench | \"it\">"  -> text(List("en", "fr", "it")),
        s"${prefix}Caption<${prefix}BOrFrench | \"it\">"  -> text(List("de", "fr", "it")),
        s"${prefix}Caption<${prefix}AOrFrench>"           -> text(List("en", "fr")),
        s"${prefix}Caption<${prefix}BOrFrench>"           -> text(List("de", "fr")),
        s"${prefix}Caption<${prefix}ALanguage | \"fr\">"  -> text(List("en", "fr"))
      ).view.mapValues(Some(_)).toMap
      val concrete                   = Try(bilingualCodec.encode(value)).map(e => SchemaWire.schemaGraphFromWit(e.graph))
      val runtime                    = Try(IntoSchema[Bilingual].graph)
      def bodies(graph: SchemaGraph) = expectedDefs.keys.toList.map(id => id -> graph.defs.get(id).map(_.body)).toMap
      val englishAsGerman            =
        value.copy(second = forged[Caption[B.Language | "fr"]](Caption(UnstructuredText.fromInline("hello", "en"))))
      assertTrue(
        runtime.map(bodies).toOption.contains(expectedDefs),
        concrete.map(bodies).toOption.contains(expectedDefs),
        concrete.toOption == runtime.toOption,
        Try(bilingualCodec.encode(englishAsGerman)).isFailure,
        Try(IntoSchema[Bilingual].toValue(englishAsGerman)).isFailure
      ) && wireAgrees(value)
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
