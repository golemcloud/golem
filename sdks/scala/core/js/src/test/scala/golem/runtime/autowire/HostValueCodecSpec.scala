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

package golem.runtime.autowire

import golem.data._
import zio.test._
import zio.durationInt

object HostValueCodecSpec extends ZIOSpecDefault {
  def spec = suite("HostValueCodecSpec")(
    test("tuple component-model encode/decode roundtrip") {
      val schema = StructuredSchema.Tuple(
        List(
          NamedElementSchema("name", ElementSchema.Component(DataType.StringType)),
          NamedElementSchema("age", ElementSchema.Component(DataType.IntType))
        )
      )
      val value = StructuredValue.Tuple(
        List(
          NamedElementValue("name", ElementValue.Component(DataValue.StringValue("alice"))),
          NamedElementValue("age", ElementValue.Component(DataValue.IntValue(30)))
        )
      )

      val encoded = HostValueEncoder.encode(schema, value).fold(err => throw new RuntimeException(err), identity)
      val decoded = HostValueDecoder.decode(schema, encoded).fold(err => throw new RuntimeException(err), identity)

      assertTrue(decoded == value)
    },
    test("multimodal encode/decode roundtrip with unstructured text and binary") {
      val schema = StructuredSchema.Multimodal(
        List(
          NamedElementSchema("text", ElementSchema.UnstructuredText(None)),
          NamedElementSchema("image", ElementSchema.UnstructuredBinary(None))
        )
      )
      val value = StructuredValue.Multimodal(
        List(
          NamedElementValue("text", ElementValue.UnstructuredText(UnstructuredTextValue.Inline("hello", Some("en")))),
          NamedElementValue(
            "image",
            ElementValue.UnstructuredBinary(
              UnstructuredBinaryValue.Inline(Array[Byte](1, 2, 3), "application/octet-stream")
            )
          )
        )
      )

      val encoded = HostValueEncoder.encode(schema, value).fold(err => throw new RuntimeException(err), identity)
      val decoded = HostValueDecoder.decode(schema, encoded).fold(err => throw new RuntimeException(err), identity)

      decoded match {
        case StructuredValue.Multimodal(elems) =>
          val decodedMap = elems.map(v => v.name -> v.value).toMap
          decodedMap.get("text") match {
            case Some(ElementValue.UnstructuredText(UnstructuredTextValue.Inline(data, lang))) =>
              assertTrue(data == "hello", lang.contains("en"))
            case other => throw new RuntimeException(s"Unexpected text value: $other")
          }
          decodedMap.get("image") match {
            case Some(ElementValue.UnstructuredBinary(UnstructuredBinaryValue.Inline(bytes, mime))) =>
              assertTrue(bytes.toSeq == Seq[Byte](1, 2, 3), mime == "application/octet-stream")
            case other => throw new RuntimeException(s"Unexpected image value: $other")
          }
        case other =>
          throw new RuntimeException(s"Unexpected decoded value: $other")
      }
    }
  ) @@ TestAspect.timeout(30.seconds)
}
