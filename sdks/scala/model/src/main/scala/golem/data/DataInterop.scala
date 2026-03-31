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

import golem.data.DataType._
import golem.data.DataValue._
import zio.blocks.chunk.Chunk
import zio.blocks.schema.{DynamicValue => DV, PrimitiveType, PrimitiveValue, Schema}
import zio.blocks.typeid.TypeId
import zio.blocks.schema.Reflect

import scala.collection.immutable.ListMap
object DataInterop {

  // TypeId constants for unsigned wrapper types
  private val ubyteTypeId: TypeId[UByte]   = TypeId.of[UByte]
  private val ushortTypeId: TypeId[UShort] = TypeId.of[UShort]
  private val uintTypeId: TypeId[UInt]     = TypeId.of[UInt]
  private val ulongTypeId: TypeId[ULong]   = TypeId.of[ULong]

  private def isUnsignedWrapper[A](reflect: Reflect.Bound[A]): Option[DataType] = {
    val tid = reflect.typeId
    if (TypeId.structurallyEqual(tid, ubyteTypeId)) Some(UByteType)
    else if (TypeId.structurallyEqual(tid, ushortTypeId)) Some(UShortType)
    else if (TypeId.structurallyEqual(tid, uintTypeId)) Some(UIntType)
    else if (TypeId.structurallyEqual(tid, ulongTypeId)) Some(ULongType)
    else None
  }

  def schemaToDataType[A](schema: Schema[A]): DataType =
    reflectToDataType(schema.reflect)

  def toData[A](value: A)(implicit schema: Schema[A]): DataValue =
    IntoDataValue[A].toData(value)

  def fromData[A](value: DataValue)(implicit schema: Schema[A]): Either[String, A] =
    FromDataValue[A].fromData(value)

  def dataTypeOf[A](implicit schema: Schema[A]): DataType =
    IntoDataType[A].dataType

  private[golem] def reflectToDataType[A](reflect: Reflect.Bound[A]): DataType =
    isUnsignedWrapper(reflect)
      .orElse(reflectToDataType_wrapper(reflect))
      .orElse(reflectToDataType_option(reflect))
      .orElse(reflectToDataType_either(reflect))
      .getOrElse(reflectToDataType_core(reflect))

  private def reflectToDataType_wrapper[A](reflect: Reflect.Bound[A]): Option[DataType] =
    reflect.asWrapperUnknown.map { unknown =>
      reflectToDataType(unknown.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]])
    }

  private def reflectToDataType_option[A](reflect: Reflect.Bound[A]): Option[DataType] =
    optionInfo(reflect).map { case (innerRef, _) =>
      Optional(reflectToDataType(innerRef))
    }

  private def reflectedTypeName(reflect: Reflect.Bound[?]): Option[String] = {
    val raw = reflect.typeId.name.stripSuffix("$")
    Option.when(raw.nonEmpty)(raw)
  }

  private def reflectToDataType_core[A](reflect: Reflect.Bound[A]): DataType =
    reflect.asPrimitive match {
      case Some(p) => primitiveToDataType(p.primitiveType)
      case None    =>
        reflect.asRecord match {
          case Some(rec) =>
            if (isTupleRecord(rec)) {
              val ordered = rec.fields.sortBy(_.name)
              TupleType(ordered.map(f => reflectToDataType(f.value.asInstanceOf[Reflect.Bound[Any]])).toList)
            } else {
              StructType(
                rec.fields.map { field =>
                  Field(
                    name = field.name,
                    dataType = reflectToDataType(field.value.asInstanceOf[Reflect.Bound[Any]]),
                    optional = false
                  )
                }.toList,
                name = reflectedTypeName(reflect)
              )
            }

          case None =>
            reflect.asSequenceUnknown match {
              case Some(seqUnknown) =>
                if (isSetTypeId(reflect.typeId))
                  SetType(reflectToDataType(seqUnknown.sequence.element.asInstanceOf[Reflect.Bound[Any]]))
                else ListType(reflectToDataType(seqUnknown.sequence.element.asInstanceOf[Reflect.Bound[Any]]))

              case None =>
                reflect.asMapUnknown match {
                  case Some(mapUnknown) =>
                    MapType(
                      reflectToDataType(mapUnknown.map.key.asInstanceOf[Reflect.Bound[Any]]),
                      reflectToDataType(mapUnknown.map.value.asInstanceOf[Reflect.Bound[Any]])
                    )

                  case None =>
                    reflect.asVariant match {
                      case Some(variant) =>
                        val typeName = reflectedTypeName(reflect)
                        val cases    = variant.cases.map { c =>
                          val payloadDt =
                            c.value.asRecord match {
                              case Some(r) if r.fields.isEmpty                                      => None
                              case Some(r) if r.fields.length == 1 && r.fields.head.name == "value" =>
                                Some(reflectToDataType(r.fields.head.value.asInstanceOf[Reflect.Bound[Any]]))
                              case Some(r) =>
                                Some(reflectToDataType(r.asInstanceOf[Reflect.Bound[Any]]))
                              case None =>
                                Some(reflectToDataType(c.value.asInstanceOf[Reflect.Bound[Any]]))
                            }
                          EnumCase(c.name, payloadDt)
                        }.toList
                        if (cases.forall(_.payload.isEmpty))
                          PureEnumType(cases.map(_.name), name = typeName)
                        else
                          EnumType(cases, name = typeName)

                      case None =>
                        if (reflect.isDynamic) StructType(Nil)
                        else throw new IllegalArgumentException(s"Unsupported schema reflect: ${reflect.nodeType}")
                    }
                }
            }
        }
    }

  private def isSetTypeId(typeId: TypeId[?]): Boolean =
    TypeId.normalize(typeId).fullName == TypeId.set.fullName

  private def isTupleRecord(rec: Reflect.Record[_root_.zio.blocks.schema.binding.Binding, _]): Boolean = {
    val names = rec.fields.map(_.name).toSet
    (rec.fields.length == 2 && names == Set("_1", "_2")) ||
    (rec.fields.length == 3 && names == Set("_1", "_2", "_3"))
  }

  /**
   * Detects an Option-like schema (Variant(None, Some(value))) and returns the
   * inner `value` field reflect.
   */
  private def optionInfo(reflect: Reflect.Bound[?]): Option[(Reflect.Bound[Any], Boolean)] =
    reflect.asVariant.flatMap { variant =>
      def simpleCaseName(name: String): String = {
        val afterDot =
          name.lastIndexOf('.') match {
            case -1 => name
            case i  => name.substring(i + 1)
          }
        if (afterDot.endsWith("$")) afterDot.dropRight(1) else afterDot
      }

      val noneCase = variant.cases.find(t => simpleCaseName(t.name) == "None")
      val someCase = variant.cases.find(t => simpleCaseName(t.name) == "Some")

      if (noneCase.isEmpty || someCase.isEmpty) None
      else {
        val someValue = someCase.get.value.asInstanceOf[Reflect.Bound[Any]]
        someValue.asRecord match {
          case Some(someRec) =>
            someRec.fieldByName("value") match {
              case Some(valueField) =>
                Some((valueField.value.asInstanceOf[Reflect.Bound[Any]], true))
              case None =>
                Some((someValue, false))
            }
          case None =>
            Some((someValue, false))
        }
      }
    }

  private def reflectToDataType_either[A](reflect: Reflect.Bound[A]): Option[DataType] =
    eitherInfo(reflect).map { case (leftRef, rightRef) =>
      val errType = leftRef match {
        case Some(r) => Some(reflectToDataType(r))
        case None    => None
      }
      val okType = rightRef match {
        case Some(r) => Some(reflectToDataType(r))
        case None    => None
      }
      ResultType(ok = okType, err = errType)
    }

  /**
   * Detects an Either-like schema (Variant(Left(value), Right(value))) and
   * returns the inner `value` field reflects for Left (err) and Right (ok).
   */
  private def eitherInfo(
    reflect: Reflect.Bound[?]
  ): Option[(Option[Reflect.Bound[Any]], Option[Reflect.Bound[Any]])] =
    reflect.asVariant.flatMap { variant =>
      def simpleCaseName(name: String): String = {
        val afterDot =
          name.lastIndexOf('.') match {
            case -1 => name
            case i  => name.substring(i + 1)
          }
        if (afterDot.endsWith("$")) afterDot.dropRight(1) else afterDot
      }

      val leftCase  = variant.cases.find(t => simpleCaseName(t.name) == "Left")
      val rightCase = variant.cases.find(t => simpleCaseName(t.name) == "Right")

      if (leftCase.isEmpty || rightCase.isEmpty || variant.cases.length != 2) None
      else {
        def extractValueRef(caseReflect: Reflect.Bound[Any]): Option[Reflect.Bound[Any]] =
          caseReflect.asRecord match {
            case Some(r) if r.fields.isEmpty                                      => None
            case Some(r) if r.fields.length == 1 && r.fields.head.name == "value" =>
              Some(r.fields.head.value.asInstanceOf[Reflect.Bound[Any]])
            case _ => Some(caseReflect)
          }
        Some(
          (
            extractValueRef(leftCase.get.value.asInstanceOf[Reflect.Bound[Any]]),
            extractValueRef(rightCase.get.value.asInstanceOf[Reflect.Bound[Any]])
          )
        )
      }
    }

  private def primitiveToDataType(pt: PrimitiveType[?]): DataType =
    pt match {
      case PrimitiveType.Unit          => UnitType
      case _: PrimitiveType.String     => StringType
      case _: PrimitiveType.Boolean    => BoolType
      case _: PrimitiveType.Byte       => ByteType
      case _: PrimitiveType.Short      => ShortType
      case _: PrimitiveType.Int        => IntType
      case _: PrimitiveType.Long       => LongType
      case _: PrimitiveType.Float      => FloatType
      case _: PrimitiveType.Double     => DoubleType
      case _: PrimitiveType.BigDecimal => BigDecimalType
      case _: PrimitiveType.BigInt     => BigDecimalType // BigInt mapped via BigDecimal encoding
      case _: PrimitiveType.UUID       => UUIDType
      case _: PrimitiveType.Char       => CharType
      case other                       =>
        throw new IllegalArgumentException(s"Unsupported primitive: ${other.getClass.getName}")
    }

  private def dynamicToDataValue[A](reflect: Reflect.Bound[A], d: DV): DataValue =
    dynamicToDataValue_unsigned(reflect, d)
      .orElse(dynamicToDataValue_wrapper(reflect, d))
      .orElse(dynamicToDataValue_option(reflect, d))
      .orElse(dynamicToDataValue_either(reflect, d))
      .orElse(dynamicToDataValue_tuple(reflect, d))
      .getOrElse(dynamicToDataValue_core(reflect, d))

  private def dynamicToDataValue_unsigned[A](reflect: Reflect.Bound[A], d: DV): Option[DataValue] = {
    val tid = reflect.typeId
    if (TypeId.structurallyEqual(tid, ubyteTypeId)) {
      // Schema.derived for AnyVal produces Record({value: Short})
      val v = extractSingleRecordPrimitive[Short](d, "UByte") { case PrimitiveValue.Short(v) => v }
      Some(UByteValue(v))
    } else if (TypeId.structurallyEqual(tid, ushortTypeId)) {
      val v = extractSingleRecordPrimitive[Int](d, "UShort") { case PrimitiveValue.Int(v) => v }
      Some(UShortValue(v))
    } else if (TypeId.structurallyEqual(tid, uintTypeId)) {
      val v = extractSingleRecordPrimitive[Long](d, "UInt") { case PrimitiveValue.Long(v) => v }
      Some(UIntValue(v))
    } else if (TypeId.structurallyEqual(tid, ulongTypeId)) {
      val v = extractSingleRecordPrimitive[BigInt](d, "ULong") {
        case PrimitiveValue.BigDecimal(v) => v.toBigInt
        case PrimitiveValue.BigInt(v)     => v
      }
      Some(ULongValue(v))
    } else None
  }

  private def extractSingleRecordPrimitive[T](d: DV, typeName: String)(pf: PartialFunction[PrimitiveValue, T]): T =
    d match {
      case DV.Record(fields) =>
        fields.find(_._1 == "value") match {
          case Some((_, DV.Primitive(pv))) =>
            pf.applyOrElse(
              pv,
              (pv: PrimitiveValue) =>
                throw new IllegalArgumentException(s"Unexpected primitive type for $typeName: $pv")
            )
          case other =>
            throw new IllegalArgumentException(s"Expected primitive 'value' field for $typeName, got $other")
        }
      case other =>
        throw new IllegalArgumentException(s"Expected Record for $typeName, got $other")
    }

  private def dynamicToDataValue_wrapper[A](reflect: Reflect.Bound[A], d: DV): Option[DataValue] =
    reflect.asWrapperUnknown.map { unknown =>
      dynamicToDataValue(unknown.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]], d)
    }

  private def dynamicToDataValue_option[A](reflect: Reflect.Bound[A], d: DV): Option[DataValue] =
    optionInfo(reflect).map { case (valueRef, usesRecordWrapper) =>
      d match {
        case DV.Variant("None", _)       => OptionalValue(None)
        case DV.Variant("Some", payload) =>
          if (usesRecordWrapper) {
            payload match {
              case DV.Record(fields) =>
                fields.find(_._1 == "value") match {
                  case Some((_, inner)) => OptionalValue(Some(dynamicToDataValue(valueRef, inner)))
                  case None             => throw new IllegalArgumentException("Option(Some) payload missing value field")
                }
              case other =>
                throw new IllegalArgumentException(s"Option(Some) payload expected record, got $other")
            }
          } else {
            OptionalValue(Some(dynamicToDataValue(valueRef, payload)))
          }
        case other =>
          throw new IllegalArgumentException(s"Option dynamic value expected Variant, got $other")
      }
    }

  private def dynamicToDataValue_either[A](reflect: Reflect.Bound[A], d: DV): Option[DataValue] =
    eitherInfo(reflect).map { case (leftRef, rightRef) =>
      d match {
        case DV.Variant("Left", payload) =>
          leftRef match {
            case Some(innerRef) =>
              payload match {
                case DV.Record(fields) =>
                  fields.find(_._1 == "value") match {
                    case Some((_, inner)) => ResultValue(Left(dynamicToDataValue(innerRef, inner)))
                    case None             => throw new IllegalArgumentException("Either(Left) payload missing value field")
                  }
                case other => ResultValue(Left(dynamicToDataValue(innerRef, other)))
              }
            case None => ResultValue(Left(NullValue))
          }
        case DV.Variant("Right", payload) =>
          rightRef match {
            case Some(innerRef) =>
              payload match {
                case DV.Record(fields) =>
                  fields.find(_._1 == "value") match {
                    case Some((_, inner)) => ResultValue(Right(dynamicToDataValue(innerRef, inner)))
                    case None             => throw new IllegalArgumentException("Either(Right) payload missing value field")
                  }
                case other => ResultValue(Right(dynamicToDataValue(innerRef, other)))
              }
            case None => ResultValue(Right(NullValue))
          }
        case other =>
          throw new IllegalArgumentException(s"Either dynamic value expected Variant(Left/Right), got $other")
      }
    }

  private def dynamicToDataValue_tuple[A](reflect: Reflect.Bound[A], d: DV): Option[DataValue] =
    reflect.asRecord.filter(isTupleRecord).map { rec =>
      d match {
        case DV.Record(fields) =>
          val map     = fields.toMap
          val ordered = rec.fields
            .sortBy(_.name)
            .map { f =>
              val dv = map.getOrElse(f.name, throw new IllegalArgumentException(s"Tuple field '${f.name}' missing"))
              dynamicToDataValue(f.value.asInstanceOf[Reflect.Bound[Any]], dv)
            }
            .toList
          TupleValue(ordered)
        case other =>
          throw new IllegalArgumentException(s"Tuple dynamic value expected record, got $other")
      }
    }

  private def dynamicToDataValue_core[A](reflect: Reflect.Bound[A], d: DV): DataValue =
    reflect.asPrimitive match {
      case Some(_) =>
        d match {
          case DV.Primitive(pv) => primitiveValue(pv)
          case other            => throw new IllegalArgumentException(s"Expected primitive dynamic value, found: $other")
        }

      case None =>
        reflect.asRecord match {
          case Some(rec) =>
            d match {
              case DV.Record(fields) =>
                val map = fields.toMap
                if (isTupleRecord(rec)) {
                  val ordered = rec.fields
                    .sortBy(_.name)
                    .map { f =>
                      val dv =
                        map.getOrElse(f.name, throw new IllegalArgumentException(s"Tuple field '${f.name}' missing"))
                      dynamicToDataValue(f.value.asInstanceOf[Reflect.Bound[Any]], dv)
                    }
                    .toList
                  TupleValue(ordered)
                } else {
                  StructValue(
                    rec.fields.map { f =>
                      val fv = map.getOrElse(f.name, throw new IllegalArgumentException(s"Missing field '${f.name}'"))
                      f.name -> dynamicToDataValue(f.value.asInstanceOf[Reflect.Bound[Any]], fv)
                    }.toMap
                  )
                }
              case other =>
                throw new IllegalArgumentException(s"Expected record dynamic value, found: $other")
            }

          case None =>
            reflect.asSequenceUnknown match {
              case Some(seqUnknown) =>
                d match {
                  case DV.Sequence(values) =>
                    val elemRef   = seqUnknown.sequence.element.asInstanceOf[Reflect.Bound[Any]]
                    val converted = values.map(v => dynamicToDataValue(elemRef, v)).toList
                    if (isSetTypeId(reflect.typeId)) SetValue(converted.toSet)
                    else ListValue(converted)
                  case other =>
                    throw new IllegalArgumentException(s"Expected sequence dynamic value, found: $other")
                }

              case None =>
                reflect.asMapUnknown match {
                  case Some(mapUnknown) =>
                    d match {
                      case DV.Map(entries) =>
                        val keyRef   = mapUnknown.map.key.asInstanceOf[Reflect.Bound[Any]]
                        val valueRef = mapUnknown.map.value.asInstanceOf[Reflect.Bound[Any]]
                        val out      = entries.map { case (k, v) =>
                          (dynamicToDataValue(keyRef, k), dynamicToDataValue(valueRef, v))
                        }.toList
                        MapValue(out)
                      case other =>
                        throw new IllegalArgumentException(s"Expected map dynamic value, found: $other")
                    }

                  case None =>
                    reflect.asVariant match {
                      case Some(variant) =>
                        val isPure = variant.cases.forall { c =>
                          c.value.asRecord.exists(_.fields.isEmpty)
                        }
                        d match {
                          case DV.Variant(name, payload) =>
                            val caseTerm = variant
                              .caseByName(name)
                              .getOrElse(
                                throw new IllegalArgumentException(s"Unknown variant case '$name'")
                              )
                            if (isPure)
                              PureEnumValue(name)
                            else {
                              val payloadRef = caseTerm.value.asInstanceOf[Reflect.Bound[Any]]
                              payloadRef.asRecord match {
                                case Some(r) if r.fields.isEmpty =>
                                  EnumValue(name, None)
                                case Some(r) if r.fields.length == 1 && r.fields.head.name == "value" =>
                                  payload match {
                                    case DV.Record(fields) =>
                                      val inner = fields
                                        .find(_._1 == "value")
                                        .map(_._2)
                                        .getOrElse(
                                          throw new IllegalArgumentException(
                                            s"Variant case '$name' missing 'value' field"
                                          )
                                        )
                                      val innerRef = r.fields.head.value.asInstanceOf[Reflect.Bound[Any]]
                                      EnumValue(name, Some(dynamicToDataValue(innerRef, inner)))
                                    case other =>
                                      EnumValue(name, Some(dynamicToDataValue(payloadRef, other)))
                                  }
                                case _ =>
                                  EnumValue(name, Some(dynamicToDataValue(payloadRef, payload)))
                              }
                            }
                          case other =>
                            throw new IllegalArgumentException(s"Expected variant dynamic value, found: $other")
                        }
                      case None =>
                        // Dynamic fallback
                        StringValue(d.toString)
                    }
                }
            }
        }
    }

  private def primitiveValue(pv: PrimitiveValue): DataValue =
    pv match {
      case PrimitiveValue.Unit          => NullValue
      case PrimitiveValue.String(v)     => StringValue(v)
      case PrimitiveValue.Boolean(v)    => BoolValue(v)
      case PrimitiveValue.Byte(v)       => ByteValue(v)
      case PrimitiveValue.Short(v)      => ShortValue(v)
      case PrimitiveValue.Int(v)        => IntValue(v)
      case PrimitiveValue.Long(v)       => LongValue(v)
      case PrimitiveValue.Float(v)      => FloatValue(v)
      case PrimitiveValue.Double(v)     => DoubleValue(v)
      case PrimitiveValue.BigDecimal(v) => BigDecimalValue(v)
      case PrimitiveValue.BigInt(v)     => BigDecimalValue(BigDecimal(v))
      case PrimitiveValue.UUID(v)       => UUIDValue(v)
      case PrimitiveValue.Char(v)       => CharValue(v)
      case other                        =>
        throw new IllegalArgumentException(s"Unsupported primitive value: ${other.getClass.getName}")
    }

  private def dataValueToDynamic[A](reflect: Reflect.Bound[A], value: DataValue): DV =
    dataValueToDynamic_unsigned(reflect, value)
      .orElse(dataValueToDynamic_wrapper(reflect, value))
      .orElse(dataValueToDynamic_option(reflect, value))
      .orElse(dataValueToDynamic_either(reflect, value))
      .getOrElse(dataValueToDynamic_tupleOrCore(reflect, value))

  private def dataValueToDynamic_unsigned[A](reflect: Reflect.Bound[A], value: DataValue): Option[DV] = {
    val tid = reflect.typeId
    if (TypeId.structurallyEqual(tid, ubyteTypeId)) {
      value match {
        case UByteValue(v) => Some(DV.Record(Chunk("value" -> DV.Primitive(PrimitiveValue.Short(v)))))
        case other         => throw new IllegalArgumentException(s"Expected UByteValue for UByte, got $other")
      }
    } else if (TypeId.structurallyEqual(tid, ushortTypeId)) {
      value match {
        case UShortValue(v) => Some(DV.Record(Chunk("value" -> DV.Primitive(PrimitiveValue.Int(v)))))
        case other          => throw new IllegalArgumentException(s"Expected UShortValue for UShort, got $other")
      }
    } else if (TypeId.structurallyEqual(tid, uintTypeId)) {
      value match {
        case UIntValue(v) => Some(DV.Record(Chunk("value" -> DV.Primitive(PrimitiveValue.Long(v)))))
        case other        => throw new IllegalArgumentException(s"Expected UIntValue for UInt, got $other")
      }
    } else if (TypeId.structurallyEqual(tid, ulongTypeId)) {
      value match {
        case ULongValue(v) =>
          Some(DV.Record(Chunk("value" -> DV.Primitive(PrimitiveValue.BigInt(v)))))
        case other => throw new IllegalArgumentException(s"Expected ULongValue for ULong, got $other")
      }
    } else None
  }

  private def dataValueToDynamic_wrapper[A](reflect: Reflect.Bound[A], value: DataValue): Option[DV] =
    reflect.asWrapperUnknown.map { unknown =>
      dataValueToDynamic(unknown.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]], value)
    }

  private def dataValueToDynamic_option[A](reflect: Reflect.Bound[A], value: DataValue): Option[DV] =
    optionInfo(reflect).map { case (innerRef, usesRecordWrapper) =>
      value match {
        case OptionalValue(None) =>
          DV.Variant("None", DV.Record(Chunk.empty))
        case OptionalValue(Some(v)) =>
          val dynInner = dataValueToDynamic(innerRef, v)
          val payload  =
            if (usesRecordWrapper) DV.Record(Chunk("value" -> dynInner))
            else dynInner
          DV.Variant("Some", payload)
        case other =>
          throw new IllegalArgumentException(s"Expected OptionalValue for Option, got $other")
      }
    }

  private def dataValueToDynamic_either[A](reflect: Reflect.Bound[A], value: DataValue): Option[DV] =
    eitherInfo(reflect).map { case (leftRef, rightRef) =>
      value match {
        case ResultValue(Left(errVal)) =>
          val dynPayload = leftRef match {
            case Some(innerRef) => DV.Record(Chunk("value" -> dataValueToDynamic(innerRef, errVal)))
            case None           => DV.Record(Chunk.empty)
          }
          DV.Variant("Left", dynPayload)
        case ResultValue(Right(okVal)) =>
          val dynPayload = rightRef match {
            case Some(innerRef) => DV.Record(Chunk("value" -> dataValueToDynamic(innerRef, okVal)))
            case None           => DV.Record(Chunk.empty)
          }
          DV.Variant("Right", dynPayload)
        case other =>
          throw new IllegalArgumentException(s"Expected ResultValue for Either, got $other")
      }
    }

  private def dataValueToDynamic_tupleOrCore[A](reflect: Reflect.Bound[A], value: DataValue): DV =
    reflect.asRecord.filter(isTupleRecord) match {
      case Some(rec) =>
        value match {
          case TupleValue(values) =>
            val orderedFields = rec.fields.sortBy(_.name)
            if (values.length != orderedFields.length)
              throw new IllegalArgumentException(
                s"Tuple arity mismatch. Expected ${orderedFields.length}, found ${values.length}"
              )
            val dynFields = orderedFields
              .zip(values)
              .map { case (f, dv) =>
                f.name -> dataValueToDynamic(f.value.asInstanceOf[Reflect.Bound[Any]], dv)
              }
              .toVector
            DV.Record(Chunk.fromIterable(dynFields))
          case other =>
            throw new IllegalArgumentException(s"Expected TupleValue for tuple, got $other")
        }
      case None =>
        dataValueToDynamic_core(reflect, value)
    }

  private def dataValueToDynamic_core[A](reflect: Reflect.Bound[A], value: DataValue): DV =
    reflect.asPrimitive match {
      case Some(p) =>
        value match {
          case NullValue =>
            // NullValue maps to Unit — verify the schema agrees.
            p.primitiveType match {
              case PrimitiveType.Unit => PrimitiveType.Unit.toDynamicValue(())
              case other              =>
                throw new IllegalArgumentException(
                  s"NullValue is only valid for Unit primitives, found: ${other.getClass.getName}"
                )
            }
          case StringValue(v) =>
            DV.Primitive(PrimitiveValue.String(v))
          case BoolValue(v) =>
            DV.Primitive(PrimitiveValue.Boolean(v))
          case CharValue(v) =>
            DV.Primitive(PrimitiveValue.Char(v))
          case ByteValue(v) =>
            DV.Primitive(PrimitiveValue.Byte(v))
          case ShortValue(v) =>
            DV.Primitive(PrimitiveValue.Short(v))
          case IntValue(v) =>
            // Backward compat: IntValue may arrive for Byte/Short schemas from old data
            p.primitiveType match {
              case _: PrimitiveType.Byte  => DV.Primitive(PrimitiveValue.Byte(v.toByte))
              case _: PrimitiveType.Short => DV.Primitive(PrimitiveValue.Short(v.toShort))
              case _                      => DV.Primitive(PrimitiveValue.Int(v))
            }
          case LongValue(v) =>
            DV.Primitive(PrimitiveValue.Long(v))
          case FloatValue(v) =>
            DV.Primitive(PrimitiveValue.Float(v))
          case DoubleValue(v) =>
            // Backward compat: DoubleValue may arrive for Float schemas from old data
            p.primitiveType match {
              case _: PrimitiveType.Float => DV.Primitive(PrimitiveValue.Float(v.toFloat))
              case _                      => DV.Primitive(PrimitiveValue.Double(v))
            }
          case BigDecimalValue(v) =>
            DV.Primitive(PrimitiveValue.BigDecimal(v))
          case UUIDValue(v) =>
            DV.Primitive(PrimitiveValue.UUID(v))
          case BytesValue(_) =>
            throw new IllegalArgumentException("Binary values are not supported by zio.blocks.schema primitives")
          case other =>
            throw new IllegalArgumentException(s"Unsupported primitive data value: $other")
        }

      case None =>
        reflect.asRecord match {
          case Some(rec) =>
            value match {
              case StructValue(fields) =>
                val map       = ListMap.from(fields)
                val dynFields = rec.fields.map { f =>
                  val dv = map.getOrElse(f.name, throw new IllegalArgumentException(s"Missing field '${f.name}'"))
                  f.name -> dataValueToDynamic(f.value.asInstanceOf[Reflect.Bound[Any]], dv)
                }.toVector
                DV.Record(Chunk.fromIterable(dynFields))
              case TupleValue(values) if isTupleRecord(rec) =>
                val orderedFields = rec.fields.sortBy(_.name)
                if (values.length != orderedFields.length)
                  throw new IllegalArgumentException(
                    s"Tuple arity mismatch. Expected ${orderedFields.length}, found ${values.length}"
                  )
                val dynFields = orderedFields
                  .zip(values)
                  .map { case (f, dv) =>
                    f.name -> dataValueToDynamic(f.value.asInstanceOf[Reflect.Bound[Any]], dv)
                  }
                  .toVector
                DV.Record(Chunk.fromIterable(dynFields))
              case other =>
                throw new IllegalArgumentException(s"Expected StructValue for record, got $other")
            }

          case None =>
            reflect.asSequenceUnknown match {
              case Some(seqUnknown) =>
                val elemRef = seqUnknown.sequence.element.asInstanceOf[Reflect.Bound[Any]]
                value match {
                  case ListValue(values) =>
                    DV.Sequence(Chunk.fromIterable(values.map(v => dataValueToDynamic(elemRef, v))))
                  case SetValue(values) =>
                    DV.Sequence(Chunk.fromIterable(values.map(v => dataValueToDynamic(elemRef, v))))
                  case other =>
                    throw new IllegalArgumentException(s"Expected ListValue/SetValue for sequence, got $other")
                }

              case None =>
                reflect.asMapUnknown match {
                  case Some(mapUnknown) =>
                    val keyRef   = mapUnknown.map.key.asInstanceOf[Reflect.Bound[Any]]
                    val valueRef = mapUnknown.map.value.asInstanceOf[Reflect.Bound[Any]]
                    value match {
                      case MapValue(entries) =>
                        val dynEntries = entries.map { case (k, v) =>
                          val dk = dataValueToDynamic(keyRef, k)
                          val dv = dataValueToDynamic(valueRef, v)
                          (dk, dv)
                        }
                        DV.Map(Chunk.fromIterable(dynEntries))
                      case other =>
                        throw new IllegalArgumentException(s"Expected MapValue for map, got $other")
                    }

                  case None =>
                    reflect.asVariant match {
                      case Some(_) =>
                        value match {
                          case PureEnumValue(caseName) =>
                            DV.Variant(caseName, DV.Record(Chunk.empty))
                          case EnumValue(caseName, maybePayload) =>
                            val payloadDyn = maybePayload match {
                              case None     => DV.Record(Chunk.empty)
                              case Some(pv) =>
                                val payloadRef =
                                  reflect.asVariant.get.caseByName(caseName).get.value.asInstanceOf[Reflect.Bound[Any]]
                                payloadRef.asRecord match {
                                  case Some(rec) if rec.fields.length == 1 && rec.fields.head.name == "value" =>
                                    DV.Record(
                                      Chunk(
                                        "value" -> dataValueToDynamic(
                                          rec.fields.head.value.asInstanceOf[Reflect.Bound[Any]],
                                          pv
                                        )
                                      )
                                    )
                                  case _ =>
                                    dataValueToDynamic(payloadRef, pv)
                                }
                            }
                            DV.Variant(caseName, payloadDyn)
                          case other =>
                            throw new IllegalArgumentException(s"Expected EnumValue for variant, got $other")
                        }
                      case None =>
                        DV.Primitive(PrimitiveValue.String(value.toString))
                    }
                }
            }
        }
    }

  trait IntoDataType[A] {
    def dataType: DataType
  }

  trait IntoDataValue[A] {
    def toData(value: A): DataValue
  }

  trait FromDataValue[A] {
    def fromData(value: DataValue): Either[String, A]
  }

  object IntoDataType {
    def apply[A](implicit ev: IntoDataType[A]): IntoDataType[A] = ev

    implicit def derived[A](implicit schema: Schema[A]): IntoDataType[A] =
      new IntoDataType[A] {
        override def dataType: DataType = schemaToDataType(schema)
      }
  }

  object IntoDataValue {
    def apply[A](implicit ev: IntoDataValue[A]): IntoDataValue[A] = ev

    implicit def derived[A](implicit schema: Schema[A]): IntoDataValue[A] =
      new IntoDataValue[A] {
        override def toData(value: A): DataValue = dynamicToDataValue(schema.reflect, schema.toDynamicValue(value))
      }
  }

  object FromDataValue {
    def apply[A](implicit ev: FromDataValue[A]): FromDataValue[A] = ev

    implicit def derived[A](implicit schema: Schema[A]): FromDataValue[A] =
      new FromDataValue[A] {
        override def fromData(value: DataValue): Either[String, A] =
          schema.fromDynamicValue(dataValueToDynamic(schema.reflect, value)).left.map(_.toString)
      }
  }
}
