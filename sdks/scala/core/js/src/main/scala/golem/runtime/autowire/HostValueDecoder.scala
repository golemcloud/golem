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

private[autowire] object HostValueDecoder {
  def decode(schema: StructuredSchema, value: JsDataValue): Either[String, StructuredValue] =
    if (js.isUndefined(value) || value == null)
      Left("Missing DataValue payload")
    else {
      val tag = value.tag
      schema match {
        case tuple: StructuredSchema.Tuple =>
          if (tag != "tuple") Left(s"Expected tuple payload, found: $tag")
          else {
            val entries = value.asInstanceOf[JsDataValueTuple].value
            decodeTupleEntries(tuple.elements, entries).map(values => StructuredValue.Tuple(values))
          }
        case multi: StructuredSchema.Multimodal =>
          if (tag != "multimodal") Left(s"Expected multimodal payload, found: $tag")
          else {
            val entries = value.asInstanceOf[JsDataValueMultimodal].value
            decodeMultimodalEntries(multi.elements, entries).map(values => StructuredValue.Multimodal(values))
          }
      }
    }

  private def decodeTupleEntries(
    schemaElements: List[NamedElementSchema],
    payload: js.Array[JsElementValue]
  ): Either[String, List[NamedElementValue]] =
    if (schemaElements.length != payload.length)
      Left(s"Structured element count mismatch. Expected ${schemaElements.length}, found ${payload.length}")
    else {
      val builder                 = List.newBuilder[NamedElementValue]
      var idx                     = 0
      var failure: Option[String] = None
      while (idx < schemaElements.length && failure.isEmpty) {
        val schemaElem   = schemaElements(idx)
        val elementValue = payload(idx)

        decodeElement(schemaElem.schema, elementValue) match {
          case Left(err)    => failure = Some(err)
          case Right(value) => builder += NamedElementValue(schemaElem.name, value)
        }
        idx += 1
      }
      failure.fold[Either[String, List[NamedElementValue]]](Right(builder.result()))(Left(_))
    }

  private def decodeMultimodalEntries(
    schemaElements: List[NamedElementSchema],
    payload: js.Array[js.Tuple2[String, JsElementValue]]
  ): Either[String, List[NamedElementValue]] =
    if (schemaElements.length != payload.length)
      Left(s"Structured element count mismatch. Expected ${schemaElements.length}, found ${payload.length}")
    else {
      val builder                 = List.newBuilder[NamedElementValue]
      var idx                     = 0
      var failure: Option[String] = None
      while (idx < schemaElements.length && failure.isEmpty) {
        val schemaElem = schemaElements(idx)
        val entry      = payload(idx)
        val name       = entry._1
        val elemValue  = entry._2

        if (name != schemaElem.name) {
          failure = Some(s"Structured element name mismatch. Expected '${schemaElem.name}', found '$name'")
        } else {
          decodeElement(schemaElem.schema, elemValue) match {
            case Left(err)    => failure = Some(err)
            case Right(value) => builder += NamedElementValue(schemaElem.name, value)
          }
        }
        idx += 1
      }
      failure.fold[Either[String, List[NamedElementValue]]](Right(builder.result()))(Left(_))
    }

  private def decodeElement(schema: ElementSchema, value: JsElementValue): Either[String, ElementValue] = {
    val tag = value.tag
    schema match {
      case ElementSchema.Component(dataType) =>
        if (tag != "component-model")
          Left(s"Expected component-model value, found: $tag")
        else {
          val witValue = value.asInstanceOf[JsElementValueComponentModel].value
          WitValueCodec.decode(dataType, witValue).map(ElementValue.Component.apply)
        }
      case ElementSchema.UnstructuredText(_) =>
        if (tag != "unstructured-text")
          Left(s"Expected unstructured-text value, found: $tag")
        else {
          val textRef = value.asInstanceOf[JsElementValueUnstructuredText].value
          decodeTextValue(textRef).map(ElementValue.UnstructuredText.apply)
        }
      case ElementSchema.UnstructuredBinary(_) =>
        if (tag != "unstructured-binary")
          Left(s"Expected unstructured-binary value, found: $tag")
        else {
          val binaryRef = value.asInstanceOf[JsElementValueUnstructuredBinary].value
          decodeBinaryValue(binaryRef).map(ElementValue.UnstructuredBinary.apply)
        }
    }
  }

  private def decodeTextValue(ref: JsTextReference): Either[String, UnstructuredTextValue] =
    ref.tag match {
      case "url" =>
        Right(UnstructuredTextValue.Url(ref.asInstanceOf[JsTextReferenceUrl].value))
      case "inline" =>
        val source   = ref.asInstanceOf[JsTextReferenceInline].value
        val data     = source.data
        val language = source.textType.toOption.map(_.languageCode)
        Right(UnstructuredTextValue.Inline(data, language))
      case other =>
        Left(s"Unsupported unstructured-text payload: $other")
    }

  private def decodeBinaryValue(ref: JsBinaryReference): Either[String, UnstructuredBinaryValue] =
    ref.tag match {
      case "url" =>
        Right(UnstructuredBinaryValue.Url(ref.asInstanceOf[JsBinaryReferenceUrl].value))
      case "inline" =>
        val source     = ref.asInstanceOf[JsBinaryReferenceInline].value
        val dataBuffer = source.data
        val mimeType   = source.binaryType.mimeType
        val bytes      = new Array[Byte](dataBuffer.length)
        var i          = 0
        while (i < dataBuffer.length) {
          bytes(i) = (dataBuffer(i) & 0xff).toByte
          i += 1
        }
        Right(UnstructuredBinaryValue.Inline(bytes, mimeType))
      case other =>
        Left(s"Unsupported unstructured-binary payload: $other")
    }
}
