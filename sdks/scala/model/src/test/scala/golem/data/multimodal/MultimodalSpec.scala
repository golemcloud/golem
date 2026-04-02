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

object MultimodalSpec extends ZIOSpecDefault {
  final case class Payload(text: String, count: Int)
  implicit val payloadSchema: Schema[Payload] = Schema.derived

  final case class Already(text: String)
  implicit val alreadySchema: GolemSchema[Already] = new GolemSchema[Already] {
    override val schema: StructuredSchema =
      StructuredSchema.Multimodal(
        List(NamedElementSchema("text", ElementSchema.Component(DataType.StringType)))
      )

    override def encode(value: Already): Either[String, StructuredValue] =
      Right(
        StructuredValue.Multimodal(
          List(NamedElementValue("text", ElementValue.Component(DataValue.StringValue(value.text))))
        )
      )

    override def decode(structured: StructuredValue): Either[String, Already] =
      structured match {
        case StructuredValue.Tuple(elements) =>
          elements.collectFirst { case NamedElementValue("text", ElementValue.Component(DataValue.StringValue(v))) =>
            Already(v)
          }.toRight("Missing text element")
        case _ =>
          Left("Expected tuple payload")
      }
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("MultimodalSpec")(
      test("derived schema uses multimodal tag") {
        val schema = implicitly[GolemSchema[Multimodal[Payload]]]
        assertTrue(schema.schema.isInstanceOf[StructuredSchema.Multimodal])
      },
      test("encodes and decodes multimodal payloads") {
        val schema = implicitly[GolemSchema[Multimodal[Payload]]]
        val value  =
          Multimodal(
            Payload(
              "hello",
              3
            )
          )

        val encoded = schema.encode(value)
        val decoded = encoded.flatMap(schema.decode)

        assert(decoded)(isRight(equalTo(value)))
      },
      test("rejects non-multimodal structured values") {
        val schema = implicitly[GolemSchema[Multimodal[Payload]]]
        val value  = StructuredValue.Tuple(
          List(NamedElementValue("value", ElementValue.Component(DataValue.StringValue("oops"))))
        )

        assert(schema.decode(value))(isLeft)
      },
      test("supports multimodal base schemas") {
        val schema  = implicitly[GolemSchema[Multimodal[Already]]]
        val value   = Multimodal(Already("ok"))
        val encoded = schema.encode(value)
        val decoded = encoded.flatMap(schema.decode)

        assert(decoded)(isRight(equalTo(value)))
      }
    )
}
