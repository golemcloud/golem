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

package golem.data

import zio.blocks.schema.Schema

/**
 * Type class for encoding/decoding Scala types to/from Golem's structured value
 * format.
 *
 * A `GolemSchema` provides:
 *   - The [[StructuredSchema]] describing the type's structure
 *   - Encoding from Scala values to [[StructuredValue]]
 *   - Decoding from [[StructuredValue]] to Scala values
 *
 * @tparam A
 *   The Scala type this schema describes
 * @see
 *   [[StructuredSchema]] for the schema representation
 * @see
 *   [[StructuredValue]] for the value representation
 */
trait GolemSchema[A] {

  /**
   * The schema describing this type's top-level structure.
   *
   * For multi-field types (tuples, case classes), this is a
   * [[StructuredSchema.Tuple]] with one [[NamedElementSchema]] per field. For
   * single-value types, this is a single-element tuple.
   */
  def schema: StructuredSchema

  /**
   * Encodes a Scala value to the top-level structured representation.
   *
   * @param value
   *   The value to encode
   * @return
   *   Right with the encoded value, or Left with an error message
   */
  def encode(value: A): Either[String, StructuredValue]

  /**
   * Decodes a top-level structured value to a Scala type.
   *
   * @param value
   *   The structured value to decode
   * @return
   *   Right with the decoded value, or Left with an error message
   */
  def decode(value: StructuredValue): Either[String, A]

  /**
   * The element-level schema for this type when used as a single parameter
   * inside a multi-parameter method or constructor.
   *
   * Unlike [[schema]] which describes the top-level payload structure,
   * `elementSchema` describes the type as a single element — analogous to Rust
   * SDK's `Schema::get_type().get_element_schema()`.
   *
   * Default: extracts the single element from a 1-element tuple schema.
   */
  def elementSchema: ElementSchema =
    schema match {
      case StructuredSchema.Tuple(elem :: Nil) => elem.schema
      case other                               =>
        throw new UnsupportedOperationException(s"Type cannot be used as a single element parameter: $other")
    }

  /**
   * Encodes a value as a single element (for use as one parameter among many).
   *
   * Default: encodes to structured value and extracts the single element.
   */
  def encodeElement(value: A): Either[String, ElementValue] =
    encode(value).flatMap {
      case StructuredValue.Tuple(NamedElementValue(_, elem) :: Nil) => Right(elem)
      case other                                                    => Left(s"Expected single-element structured value, found: $other")
    }

  /**
   * Decodes a single element value back to a Scala type.
   *
   * Default: wraps in a single-element structured value and decodes.
   */
  def decodeElement(value: ElementValue): Either[String, A] =
    decode(StructuredValue.single(value))
}

/**
 * Companion object providing implicit derivation and factory methods.
 */
object GolemSchema {

  implicit val unitGolemSchema: GolemSchema[Unit] =
    new GolemSchema[Unit] {
      override val schema: StructuredSchema =
        StructuredSchema.Tuple(Nil)

      override def encode(value: Unit): Either[String, StructuredValue] =
        Right(StructuredValue.Tuple(Nil))

      override def decode(value: StructuredValue): Either[String, Unit] =
        value match {
          case StructuredValue.Tuple(elements) if elements.isEmpty => Right(())
          case other                                               =>
            Left(s"Expected empty tuple for Unit payload, found: ${other.getClass.getSimpleName}")
        }

      override val elementSchema: ElementSchema =
        ElementSchema.Component(DataType.TupleType(Nil))

      override def encodeElement(value: Unit): Either[String, ElementValue] =
        Right(ElementValue.Component(DataValue.TupleValue(Nil)))

      override def decodeElement(value: ElementValue): Either[String, Unit] =
        Right(())
    }

  /**
   * Summons a GolemSchema instance for the given type.
   *
   * @tparam A
   *   The type to get a schema for
   * @return
   *   The GolemSchema instance
   */
  def apply[A](implicit codec: GolemSchema[A]): GolemSchema[A] = codec

  // ---------------------------------------------------------------------------
  // Convenience schemas
  //
  // Tuple schemas are provided so constructor inputs can be encoded without extra boilerplate.
  // ---------------------------------------------------------------------------

  implicit def tuple2GolemSchema[A: Schema, B: Schema]: GolemSchema[(A, B)] =
    new GolemSchema[(A, B)] {
      private val aSchema = implicitly[Schema[A]]
      private val bSchema = implicitly[Schema[B]]

      private val aDt = DataInterop.schemaToDataType(aSchema)
      private val bDt = DataInterop.schemaToDataType(bSchema)

      override val schema: StructuredSchema =
        StructuredSchema.Tuple(
          List(
            NamedElementSchema("arg0", ElementSchema.Component(aDt)),
            NamedElementSchema("arg1", ElementSchema.Component(bDt))
          )
        )

      override def encode(value: (A, B)): Either[String, StructuredValue] = {
        val (a, b) = value
        val av     = DataInterop.toData[A](a)(aSchema)
        val bv     = DataInterop.toData[B](b)(bSchema)
        Right(
          StructuredValue.Tuple(
            List(
              NamedElementValue("arg0", ElementValue.Component(av)),
              NamedElementValue("arg1", ElementValue.Component(bv))
            )
          )
        )
      }

      override def decode(value: StructuredValue): Either[String, (A, B)] =
        value match {
          case StructuredValue.Tuple(elements) =>
            def find(name: String): Either[String, DataValue] =
              elements
                .find(_.name == name)
                .toRight(s"Tuple2 payload missing field '$name'")
                .flatMap {
                  case NamedElementValue(_, ElementValue.Component(dv)) => Right(dv)
                  case other                                            =>
                    Left(
                      s"Tuple2 payload field '$name' must be component-model, found: ${other.value.getClass.getSimpleName}"
                    )
                }

            for {
              av <- find("arg0")
              bv <- find("arg1")
              a  <- DataInterop.fromData[A](av)(aSchema)
              b  <- DataInterop.fromData[B](bv)(bSchema)
            } yield (a, b)

          case StructuredValue.Multimodal(_) =>
            Left("Multimodal payload cannot be decoded as component-model value")
        }

      override val elementSchema: ElementSchema =
        ElementSchema.Component(DataType.TupleType(List(aDt, bDt)))

      override def encodeElement(value: (A, B)): Either[String, ElementValue] = {
        val (a, b) = value
        val av     = DataInterop.toData[A](a)(aSchema)
        val bv     = DataInterop.toData[B](b)(bSchema)
        Right(ElementValue.Component(DataValue.TupleValue(List(av, bv))))
      }

      override def decodeElement(value: ElementValue): Either[String, (A, B)] =
        value match {
          case ElementValue.Component(DataValue.TupleValue(List(av, bv))) =>
            for {
              a <- DataInterop.fromData[A](av)(aSchema)
              b <- DataInterop.fromData[B](bv)(bSchema)
            } yield (a, b)
          case other =>
            Left(s"Expected component TupleValue for Tuple2, found: ${other.getClass.getSimpleName}")
        }
    }

  implicit def tuple3GolemSchema[A: Schema, B: Schema, C: Schema]: GolemSchema[(A, B, C)] =
    // See tuple2GolemSchema above for rationale.
    new GolemSchema[(A, B, C)] {
      private val aSchema = implicitly[Schema[A]]
      private val bSchema = implicitly[Schema[B]]
      private val cSchema = implicitly[Schema[C]]

      private val aDt = DataInterop.schemaToDataType(aSchema)
      private val bDt = DataInterop.schemaToDataType(bSchema)
      private val cDt = DataInterop.schemaToDataType(cSchema)

      override val schema: StructuredSchema =
        StructuredSchema.Tuple(
          List(
            NamedElementSchema("arg0", ElementSchema.Component(aDt)),
            NamedElementSchema("arg1", ElementSchema.Component(bDt)),
            NamedElementSchema("arg2", ElementSchema.Component(cDt))
          )
        )

      override def encode(value: (A, B, C)): Either[String, StructuredValue] = {
        val (a, b, c) = value
        val av        = DataInterop.toData[A](a)(aSchema)
        val bv        = DataInterop.toData[B](b)(bSchema)
        val cv        = DataInterop.toData[C](c)(cSchema)
        Right(
          StructuredValue.Tuple(
            List(
              NamedElementValue("arg0", ElementValue.Component(av)),
              NamedElementValue("arg1", ElementValue.Component(bv)),
              NamedElementValue("arg2", ElementValue.Component(cv))
            )
          )
        )
      }

      override def decode(value: StructuredValue): Either[String, (A, B, C)] =
        value match {
          case StructuredValue.Tuple(elements) =>
            def find(name: String): Either[String, DataValue] =
              elements
                .find(_.name == name)
                .toRight(s"Tuple3 payload missing field '$name'")
                .flatMap {
                  case NamedElementValue(_, ElementValue.Component(dv)) => Right(dv)
                  case other                                            =>
                    Left(
                      s"Tuple3 payload field '$name' must be component-model, found: ${other.value.getClass.getSimpleName}"
                    )
                }

            for {
              av <- find("arg0")
              bv <- find("arg1")
              cv <- find("arg2")
              a  <- DataInterop.fromData[A](av)(aSchema)
              b  <- DataInterop.fromData[B](bv)(bSchema)
              c  <- DataInterop.fromData[C](cv)(cSchema)
            } yield (a, b, c)

          case StructuredValue.Multimodal(_) =>
            Left("Multimodal payload cannot be decoded as component-model value")
        }

      override val elementSchema: ElementSchema =
        ElementSchema.Component(DataType.TupleType(List(aDt, bDt, cDt)))

      override def encodeElement(value: (A, B, C)): Either[String, ElementValue] = {
        val (a, b, c) = value
        val av        = DataInterop.toData[A](a)(aSchema)
        val bv        = DataInterop.toData[B](b)(bSchema)
        val cv        = DataInterop.toData[C](c)(cSchema)
        Right(ElementValue.Component(DataValue.TupleValue(List(av, bv, cv))))
      }

      override def decodeElement(value: ElementValue): Either[String, (A, B, C)] =
        value match {
          case ElementValue.Component(DataValue.TupleValue(List(av, bv, cv))) =>
            for {
              a <- DataInterop.fromData[A](av)(aSchema)
              b <- DataInterop.fromData[B](bv)(bSchema)
              c <- DataInterop.fromData[C](cv)(cSchema)
            } yield (a, b, c)
          case other =>
            Left(s"Expected component TupleValue for Tuple3, found: ${other.getClass.getSimpleName}")
        }
    }

  /**
   * Derives a GolemSchema from ZIO Blocks Schema.
   *
   * This is the primary derivation path - any type with a
   * `zio.blocks.schema.Schema` automatically gets a `GolemSchema` via this
   * implicit.
   */
  implicit def fromBlocksSchema[A](implicit baseSchema: Schema[A]): GolemSchema[A] = new GolemSchema[A] {
    private val dataType = DataInterop.schemaToDataType(baseSchema)

    override val schema: StructuredSchema =
      StructuredSchema.single(ElementSchema.Component(dataType))

    override def encode(value: A): Either[String, StructuredValue] = {
      val dataValue = DataInterop.toData[A](value)(baseSchema)
      Right(StructuredValue.single(ElementValue.Component(dataValue)))
    }

    override def decode(value: StructuredValue): Either[String, A] =
      value match {
        case StructuredValue.Tuple(elements) =>
          elements.headOption match {
            case Some(NamedElementValue(_, ElementValue.Component(dataValue))) =>
              DataInterop.fromData[A](dataValue)(baseSchema)
            case Some(other) =>
              Left(s"Expected component-model value, found: ${other.value.getClass.getSimpleName}")
            case None =>
              Left("Tuple payload missing component value")
          }
        case StructuredValue.Multimodal(_) =>
          Left("Multimodal payload cannot be decoded as component-model value")
      }

    override val elementSchema: ElementSchema =
      ElementSchema.Component(dataType)

    override def encodeElement(value: A): Either[String, ElementValue] =
      Right(ElementValue.Component(DataInterop.toData[A](value)(baseSchema)))

    override def decodeElement(value: ElementValue): Either[String, A] =
      value match {
        case ElementValue.Component(dataValue) =>
          DataInterop.fromData[A](dataValue)(baseSchema)
        case other =>
          Left(s"Expected component-model value, found: ${other.getClass.getSimpleName}")
      }
  }
}
