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

/**
 * A sealed trait representing a single modality item in a multimodal payload.
 *
 * @tparam A
 *   The custom data type (use `Nothing` for basic text+binary only)
 */
sealed trait Modality[+A] extends Product with Serializable

object Modality {

  /** A basic modality item — text or binary only. */
  sealed trait Basic extends Modality[Nothing]

  /** A text modality item with unconstrained language. */
  final case class Text(value: UnstructuredTextValue) extends Basic

  /** A binary modality item with unconstrained MIME type. */
  final case class Binary(value: UnstructuredBinaryValue) extends Basic

  /** A custom structured data modality item. */
  final case class Custom[A](value: A) extends Modality[A]

  /** Creates an inline text item. */
  def text(data: String, languageCode: Option[String] = None): Basic =
    Text(UnstructuredTextValue.Inline(data, languageCode))

  /** Creates a URL-referenced text item. */
  def textUrl(url: String): Basic =
    Text(UnstructuredTextValue.Url(url))

  /** Creates an inline binary item. */
  def binary(data: Array[Byte], mimeType: String): Basic =
    Binary(UnstructuredBinaryValue.Inline(data, mimeType))

  /** Creates a URL-referenced binary item. */
  def binaryUrl(url: String): Basic =
    Binary(UnstructuredBinaryValue.Url(url))

  /** Creates a custom structured data item. */
  def custom[A](value: A): Modality[A] =
    Custom(value)
}

/**
 * Type class for types that can serve as elements within a [[MultimodalItems]]
 * container.
 *
 * Each implementor provides:
 *   - The set of possible modality schemas (name → ElementSchema)
 *   - Encoding/decoding individual items to/from named element values
 */
trait ModalityCodec[A] {

  /** The named element schemas for all possible modalities. */
  def schemas: List[NamedElementSchema]

  /** Encode a single item to a named element value. */
  def encodeItem(value: A): Either[String, NamedElementValue]

  /** Decode a single item from a named element value. */
  def decodeItem(named: NamedElementValue): Either[String, A]
}

object ModalityCodec {

  implicit val basicCodec: ModalityCodec[Modality.Basic] = new ModalityCodec[Modality.Basic] {
    override val schemas: List[NamedElementSchema] = List(
      NamedElementSchema("Text", ElementSchema.UnstructuredText(None)),
      NamedElementSchema("Binary", ElementSchema.UnstructuredBinary(None))
    )

    override def encodeItem(value: Modality.Basic): Either[String, NamedElementValue] =
      value match {
        case Modality.Text(v)   => Right(NamedElementValue("Text", ElementValue.UnstructuredText(v)))
        case Modality.Binary(v) => Right(NamedElementValue("Binary", ElementValue.UnstructuredBinary(v)))
      }

    override def decodeItem(named: NamedElementValue): Either[String, Modality.Basic] =
      (named.name, named.value) match {
        case ("Text", ElementValue.UnstructuredText(v))     => Right(Modality.Text(v))
        case ("Binary", ElementValue.UnstructuredBinary(v)) => Right(Modality.Binary(v))
        case (name, _)                                      => Left(s"Unknown basic modality: $name")
      }
  }

  implicit def customCodec[A](implicit inner: GolemSchema[A]): ModalityCodec[Modality[A]] =
    new ModalityCodec[Modality[A]] {
      private val basicC = basicCodec

      override val schemas: List[NamedElementSchema] =
        basicC.schemas :+ NamedElementSchema("Custom", inner.elementSchema)

      override def encodeItem(value: Modality[A]): Either[String, NamedElementValue] =
        value match {
          case b: Modality.Basic  => basicC.encodeItem(b)
          case Modality.Custom(v) => inner.encodeElement(v).map(NamedElementValue("Custom", _))
        }

      override def decodeItem(named: NamedElementValue): Either[String, Modality[A]] =
        named.name match {
          case "Text" | "Binary" => basicC.decodeItem(named).map(identity[Modality[A]])
          case "Custom"          => inner.decodeElement(named.value).map(Modality.Custom(_))
          case other             => Left(s"Unknown modality: $other")
        }
    }
}

/**
 * A runtime-sized multimodal payload — a sequence of modality items.
 *
 * This is the list-based counterpart to [[Multimodal]], which lifts a fixed
 * case class into multimodal schema. Use `MultimodalItems` when the number of
 * items varies at runtime.
 *
 * Equivalent to Rust SDK's `Multimodal` / `MultimodalAdvanced<T>` /
 * `MultimodalCustom<T>`.
 *
 * @tparam A
 *   The modality item type
 * @param items
 *   The list of modality items
 */
final case class MultimodalItems[+A](items: List[A])

object MultimodalItems {

  /** Basic multimodal: text and binary items only. */
  type Basic = MultimodalItems[Modality.Basic]

  /** Custom multimodal: text, binary, and custom structured items. */
  type WithCustom[A] = MultimodalItems[Modality[A]]

  /** Creates a basic multimodal payload from text/binary items. */
  def basic(items: Modality.Basic*): Basic = MultimodalItems(items.toList)

  /** Creates a custom multimodal payload. */
  def withCustom[A](items: Modality[A]*): WithCustom[A] = MultimodalItems(items.toList)

  implicit def multimodalItemsSchema[A](implicit codec: ModalityCodec[A]): GolemSchema[MultimodalItems[A]] =
    new GolemSchema[MultimodalItems[A]] {
      override val schema: StructuredSchema =
        StructuredSchema.Multimodal(codec.schemas)

      override def encode(value: MultimodalItems[A]): Either[String, StructuredValue] = {
        val builder               = List.newBuilder[NamedElementValue]
        var error: Option[String] = None
        val iter                  = value.items.iterator
        while (iter.hasNext && error.isEmpty) {
          codec.encodeItem(iter.next()) match {
            case Left(err)  => error = Some(err)
            case Right(nev) => builder += nev
          }
        }
        error match {
          case Some(err) => Left(err)
          case None      => Right(StructuredValue.Multimodal(builder.result()))
        }
      }

      override def decode(structured: StructuredValue): Either[String, MultimodalItems[A]] =
        structured match {
          case StructuredValue.Multimodal(elements) =>
            val builder               = List.newBuilder[A]
            var error: Option[String] = None
            val iter                  = elements.iterator
            while (iter.hasNext && error.isEmpty) {
              codec.decodeItem(iter.next()) match {
                case Left(err) => error = Some(err)
                case Right(a)  => builder += a
              }
            }
            error match {
              case Some(err) => Left(err)
              case None      => Right(MultimodalItems(builder.result()))
            }
          case other =>
            Left(s"Expected multimodal structured value, found $other")
        }

      override def elementSchema: ElementSchema =
        throw new UnsupportedOperationException("MultimodalItems cannot be used as a single element parameter")

      override def encodeElement(value: MultimodalItems[A]): Either[String, ElementValue] =
        Left("MultimodalItems cannot be encoded as a single element; it must be the sole parameter")

      override def decodeElement(value: ElementValue): Either[String, MultimodalItems[A]] =
        Left("MultimodalItems cannot be decoded from a single element; it must be the sole parameter")
    }
}
