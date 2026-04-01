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
import golem.host.js._

import scala.scalajs.js

private[autowire] object HostValueEncoder {
  def encode(schema: StructuredSchema, value: StructuredValue): Either[String, JsDataValue] =
    (schema, value) match {
      case (StructuredSchema.Tuple(schemaElements), StructuredValue.Tuple(valueElements)) =>
        encodeTupleEntries(schemaElements, valueElements).map(JsDataValue.tuple)
      case (StructuredSchema.Multimodal(schemaElements), StructuredValue.Multimodal(valueElements)) =>
        encodeMultimodalEntries(schemaElements, valueElements).map(JsDataValue.multimodal)
      case (StructuredSchema.Tuple(_), _) =>
        Left("Structured value mismatch: expected tuple payload")
      case (StructuredSchema.Multimodal(_), _) =>
        Left("Structured value mismatch: expected multimodal payload")
    }

  private def encodeTupleEntries(
    schemaElements: List[NamedElementSchema],
    valueElements: List[NamedElementValue]
  ): Either[String, js.Array[JsElementValue]] =
    if (schemaElements.length != valueElements.length)
      Left(s"Structured element count mismatch. Expected ${schemaElements.length}, found ${valueElements.length}")
    else {
      val array = new js.Array[JsElementValue]()
      schemaElements
        .zip(valueElements)
        .foldLeft[Either[String, Unit]](Right(())) { case (acc, (schemaElem, valueElem)) =>
          acc.flatMap { _ =>
            if (schemaElem.name != valueElem.name)
              Left(s"Structured element name mismatch. Expected '${schemaElem.name}', found '${valueElem.name}'")
            else
              encodeElement(schemaElem.schema, valueElem.value).map { encoded =>
                array.push(encoded)
              }
          }
        }
        .map(_ => array)
    }

  private def encodeMultimodalEntries(
    schemaElements: List[NamedElementSchema],
    valueElements: List[NamedElementValue]
  ): Either[String, js.Array[js.Tuple2[String, JsElementValue]]] =
    if (schemaElements.length != valueElements.length)
      Left(s"Structured element count mismatch. Expected ${schemaElements.length}, found ${valueElements.length}")
    else {
      val array = new js.Array[js.Tuple2[String, JsElementValue]]()
      schemaElements
        .zip(valueElements)
        .foldLeft[Either[String, Unit]](Right(())) { case (acc, (schemaElem, valueElem)) =>
          acc.flatMap { _ =>
            if (schemaElem.name != valueElem.name)
              Left(s"Structured element name mismatch. Expected '${schemaElem.name}', found '${valueElem.name}'")
            else
              encodeElement(schemaElem.schema, valueElem.value).map { encoded =>
                array.push(js.Tuple2(valueElem.name, encoded))
              }
          }
        }
        .map(_ => array)
    }

  private def encodeElement(schema: ElementSchema, value: ElementValue): Either[String, JsElementValue] =
    (schema, value) match {
      case (ElementSchema.Component(dataType), ElementValue.Component(dataValue)) =>
        WitValueBuilder.build(dataType, dataValue).map { witValue =>
          JsElementValue.componentModel(witValue)
        }
      case (ElementSchema.UnstructuredText(_), ElementValue.UnstructuredText(textValue)) =>
        Right(JsElementValue.unstructuredText(encodeTextValue(textValue)))
      case (ElementSchema.UnstructuredBinary(_), ElementValue.UnstructuredBinary(binaryValue)) =>
        Right(JsElementValue.unstructuredBinary(encodeBinaryValue(binaryValue)))
      case (expected, found) =>
        Left(s"Element schema/value mismatch. Expected $expected, found $found")
    }

  private def encodeTextValue(value: UnstructuredTextValue): JsTextReference =
    value match {
      case UnstructuredTextValue.Url(url) =>
        JsTextReference.url(url)
      case UnstructuredTextValue.Inline(data, language) =>
        val textType: js.UndefOr[JsTextType] =
          language.fold[js.UndefOr[JsTextType]](js.undefined)(code => JsTextType(code))
        JsTextReference.inline(JsTextSource(data, textType))
    }

  private def encodeBinaryValue(value: UnstructuredBinaryValue): JsBinaryReference =
    value match {
      case UnstructuredBinaryValue.Url(url) =>
        JsBinaryReference.url(url)
      case UnstructuredBinaryValue.Inline(data, mimeType) =>
        val typedArray = new js.typedarray.Uint8Array(data.length)
        var idx        = 0
        while (idx < data.length) {
          typedArray(idx) = ((data(idx) & 0xff).toShort)
          idx += 1
        }
        JsBinaryReference.inline(JsBinarySource(typedArray, JsBinaryType(mimeType)))
    }
}
