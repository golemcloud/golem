/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.data.multimodal

import golem.data._
import zio.blocks.schema.Schema
import zio.test._
import zio.test.Assertion._

object MultimodalItemsSpec extends ZIOSpecDefault {

  final case class MyData(x: String, y: Int)
  implicit val myDataSchema: Schema[MyData] = Schema.derived

  override def spec: Spec[TestEnvironment, Any] =
    suite("MultimodalItemsSpec")(
      suite("Basic modality")(
        test("schema uses multimodal tag with Text and Binary elements") {
          val schema = implicitly[GolemSchema[MultimodalItems.Basic]]
          schema.schema match {
            case StructuredSchema.Multimodal(elements) =>
              assertTrue(
                elements.length == 2,
                elements(0).name == "Text",
                elements(0).schema.isInstanceOf[ElementSchema.UnstructuredText],
                elements(1).name == "Binary",
                elements(1).schema.isInstanceOf[ElementSchema.UnstructuredBinary]
              )
            case other =>
              assertTrue(false) ?? s"Expected multimodal schema, found $other"
          }
        },
        test("encode/decode roundtrip for basic items") {
          val schema = implicitly[GolemSchema[MultimodalItems.Basic]]
          val value  = MultimodalItems.basic(
            Modality.text("hello"),
            Modality.binary(Array[Byte](1, 2, 3), "image/png"),
            Modality.textUrl("https://example.com/text")
          )

          val encoded = schema.encode(value)
          val decoded = encoded.flatMap(schema.decode)

          assert(decoded)(isRight(hasField("items", _.items, hasSize(equalTo(3))))) &&
          assert(decoded.map(_.items(0)))(isRight(isSubtype[Modality.Text](anything))) &&
          assert(decoded.map(_.items(1)))(isRight(isSubtype[Modality.Binary](anything))) &&
          assert(decoded.map(_.items(2)))(isRight(isSubtype[Modality.Text](anything)))
        },
        test("rejects non-multimodal structured values") {
          val schema = implicitly[GolemSchema[MultimodalItems.Basic]]
          val value  = StructuredValue.Tuple(
            List(NamedElementValue("value", ElementValue.Component(DataValue.StringValue("oops"))))
          )

          assert(schema.decode(value))(isLeft)
        },
        test("empty items roundtrip") {
          val schema = implicitly[GolemSchema[MultimodalItems.Basic]]
          val value  = MultimodalItems.basic()

          val encoded = schema.encode(value)
          val decoded = encoded.flatMap(schema.decode)

          assert(decoded)(isRight(equalTo(MultimodalItems(Nil))))
        }
      ),
      suite("Custom modality")(
        test("schema uses multimodal tag with Text, Binary, and Custom elements") {
          val schema = implicitly[GolemSchema[MultimodalItems.WithCustom[MyData]]]
          schema.schema match {
            case StructuredSchema.Multimodal(elements) =>
              assertTrue(
                elements.length == 3,
                elements(0).name == "Text",
                elements(1).name == "Binary",
                elements(2).name == "Custom",
                elements(2).schema.isInstanceOf[ElementSchema.Component]
              )
            case other =>
              assertTrue(false) ?? s"Expected multimodal schema, found $other"
          }
        },
        test("encode/decode roundtrip for custom items") {
          val schema = implicitly[GolemSchema[MultimodalItems.WithCustom[MyData]]]
          val value  = MultimodalItems.withCustom[MyData](
            Modality.text("hello"),
            Modality.custom(MyData("world", 42)),
            Modality.binary(Array[Byte](10, 20), "audio/wav")
          )

          val encoded = schema.encode(value)
          val decoded = encoded.flatMap(schema.decode)

          assert(decoded)(isRight(hasField("items", _.items, hasSize(equalTo(3))))) &&
          assert(decoded.map(_.items(1)))(isRight(isSubtype[Modality.Custom[MyData]](anything)))
        }
      ),
      suite("Modality factory methods")(
        test("text creates inline text") {
          val m = Modality.text("hello", Some("en"))
          assertTrue(m == Modality.Text(UnstructuredTextValue.Inline("hello", Some("en"))))
        },
        test("textUrl creates url text") {
          val m = Modality.textUrl("https://example.com")
          assertTrue(m == Modality.Text(UnstructuredTextValue.Url("https://example.com")))
        },
        test("binary creates inline binary") {
          val m = Modality.binary(Array[Byte](1, 2), "image/png")
          m match {
            case Modality.Binary(UnstructuredBinaryValue.Inline(data, mime)) =>
              assertTrue(data.toList == List[Byte](1, 2), mime == "image/png")
            case other =>
              assertTrue(false) ?? s"Expected inline binary, got $other"
          }
        },
        test("binaryUrl creates url binary") {
          val m = Modality.binaryUrl("https://example.com/img.png")
          assertTrue(m == Modality.Binary(UnstructuredBinaryValue.Url("https://example.com/img.png")))
        }
      )
    )
}
