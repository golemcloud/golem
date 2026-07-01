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

import golem.Uuid
import golem.{UByte, UInt, ULong, UShort}
import zio.blocks.chunk.Chunk
import zio.blocks.schema.{DynamicValue => DV, Modifier, PrimitiveType, PrimitiveValue, Reflect, Schema}
import zio.blocks.typeid.{TypeId, TypeIdPrinter, TypeRepr}

import java.time.{Duration => JDuration, Instant}
import scala.collection.mutable
import scala.util.control.NonFatal

/**
 * Structural derivation engine for the `golem:core/types@2.0.0` schema model.
 *
 * Walks the zio-blocks [[Reflect]] tree **directly** to build a recursive
 * [[SchemaGraph]] (nominal records / variants / enums are registered into a
 * [[SchemaBuilder]] keyed on a stable, generic-aware `TypeId`, so recursive and
 * mutually-recursive types close via reserve/commit), and uses zio-blocks
 * [[DV DynamicValue]] **only** as the value accessor / reconstructor — never as
 * the source of structure.
 *
 * The engine is host-agnostic: no `js.*` imports, Scala 2.13 + 3 clean.
 *
 * Rich types are intentionally NOT handled here (see `scala-sdk.md`, Slice 2
 * scope): the only `Schema`-bearing rich wrapper, `golem.config.Secret`, is
 * rejected loudly rather than silently unwrapped.
 */
private[golem] object Derivation {

  // Canonical id of the cross-SDK UUID record, byte-identical to the Rust SDK.
  private final val UuidTypeId  = "uuid.Uuid"
  private final val UuidName    = "uuid"
  private final val SecretFqn   = "golem.config.Secret"
  private final val PathFqn     = "golem.schema.GolemPath"
  private final val UrlFqn      = "golem.schema.Url"
  private final val QuantityFqn = "golem.schema.Quantity"

  private val ubyteTypeId: TypeId[UByte]        = TypeId.of[UByte]
  private val ushortTypeId: TypeId[UShort]      = TypeId.of[UShort]
  private val uintTypeId: TypeId[UInt]          = TypeId.of[UInt]
  private val ulongTypeId: TypeId[ULong]        = TypeId.of[ULong]
  private val uuidTypeId: TypeId[Uuid]          = TypeId.of[Uuid]
  private val instantTypeId: TypeId[Instant]    = TypeId.of[Instant]
  private val durationTypeId: TypeId[JDuration] = TypeId.of[JDuration]

  // Inclusive upper bounds of the WIT unsigned ranges.
  private final val MaxU8: Long  = 0xffL
  private final val MaxU16: Long = 0xffffL
  private final val MaxU32: Long = 0xffffffffL
  private val MaxU64: BigInt     = (BigInt(1) << 64) - 1

  // ---------------------------------------------------------------------------
  // Public entry points
  // ---------------------------------------------------------------------------

  /** Build the self-contained schema graph for a type. */
  def graphOf[A](schema: Schema[A]): SchemaGraph = {
    val ctx  = new Ctx
    val root = reflectToSchema(schema.reflect, ctx)
    ctx.builder.buildGraph(root)
  }

  /** Convert a Scala value into its structural [[SchemaValue]]. */
  def toValue[A](schema: Schema[A], value: A): SchemaValue =
    try dynamicToSchemaValue(schema.reflect, schema.toDynamicValue(value))
    catch {
      case e: SchemaEncodeError => throw e
      case NonFatal(e)          => throw SchemaEncodeError(Option(e.getMessage).getOrElse(e.toString))
    }

  /** Reconstruct a Scala value from a structural [[SchemaValue]]. */
  def fromValue[A](schema: Schema[A], value: SchemaValue): Either[FromSchemaError, A] =
    try
      schema
        .fromDynamicValue(schemaValueToDynamic(schema.reflect, value))
        .left
        .map(err => FromSchemaError(err.toString))
    catch {
      case e: FromSchemaError => Left(e)
      case NonFatal(e)        => Left(FromSchemaError(Option(e.getMessage).getOrElse(e.toString)))
    }

  // ---------------------------------------------------------------------------
  // Derivation context: builder + same-builder TypeId conflict detection
  // ---------------------------------------------------------------------------

  private final class Ctx {
    val builder: SchemaBuilder               = new SchemaBuilder
    val seen: mutable.Map[String, TypeId[?]] = mutable.Map.empty

    /**
     * Register a nominal type, detecting same-builder id conflicts: if `id` was
     * already registered by a structurally-different `TypeId`, fail loudly
     * rather than silently aliasing the two types together.
     */
    def register(id: String, tid: TypeId[?], name: Option[String], build: () => SchemaType): SchemaType =
      seen.get(id) match {
        case Some(prev) if !TypeId.structurallyEqual(prev, tid) =>
          throw SchemaConflictError(
            id,
            Some(s"type id '$id' maps to two distinct types: '${prev.fullName}' and '${tid.fullName}'")
          )
        case Some(_) =>
          builder.ref(id)
        case None =>
          seen.update(id, tid)
          builder.reserve(id, name)
          builder.commit(id, build(), name)
          builder.ref(id)
      }
  }

  // ---------------------------------------------------------------------------
  // Stable, generic-aware TypeId rendering
  // ---------------------------------------------------------------------------

  private def stableId(id: TypeId[?]): String = {
    val n = TypeId.normalize(id)
    if (n.typeArgs.isEmpty) n.fullName
    else n.fullName + "<" + n.typeArgs.map(renderRepr).mkString(",") + ">"
  }

  private def renderRepr(r: TypeRepr): String =
    r match {
      case TypeRepr.Ref(id)              => stableId(id)
      case TypeRepr.Applied(tycon, args) => renderRepr(tycon) + "<" + args.map(renderRepr).mkString(",") + ">"
      case other                         => TypeIdPrinter.render(other)
    }

  private def shortName(reflect: Reflect.Bound[?]): Option[String] = {
    val raw = reflect.typeId.name.stripSuffix("$")
    Option.when(raw.nonEmpty)(raw)
  }

  // ---------------------------------------------------------------------------
  // Shared classification helpers
  // ---------------------------------------------------------------------------

  private def failIfUnsupported(reflect: Reflect.Bound[?]): Unit =
    if (TypeId.normalize(reflect.typeId).fullName == SecretFqn)
      throw SchemaEncodeError(
        "golem.config.Secret is not supported by structural schema derivation; " +
          "do not derive a schema for it directly."
      )

  private def isUuid(reflect: Reflect.Bound[?]): Boolean =
    TypeId.structurallyEqual(reflect.typeId, uuidTypeId)

  private def normalizedName(reflect: Reflect.Bound[?]): String =
    TypeId.normalize(reflect.typeId).fullName

  private def isPath(reflect: Reflect.Bound[?]): Boolean                    = normalizedName(reflect) == PathFqn
  private def isUrl(reflect: Reflect.Bound[?]): Boolean                     = normalizedName(reflect) == UrlFqn
  private def isInstant(reflect: Reflect.Bound[?]): Boolean                 = TypeId.structurallyEqual(reflect.typeId, instantTypeId)
  private def isDuration(reflect: Reflect.Bound[?]): Boolean                = TypeId.structurallyEqual(reflect.typeId, durationTypeId)
  private def quantitySpec(reflect: Reflect.Bound[?]): Option[QuantitySpec] =
    if (normalizedName(reflect) == QuantityFqn)
      Some(
        reflect.modifiers.collectFirst { case Modifier.config(Quantity.SpecConfigKey, value) => value }
          .flatMap(Quantity.decodeSpec)
          .getOrElse(throw missingQuantitySpecError)
      )
    else None

  private def missingQuantitySpecError: SchemaEncodeError =
    SchemaEncodeError(
      "Quantity[U] requires Quantity.schema[U] with an implicit QuantityUnit[U]; do not use Schema.derived[Quantity[U]] directly"
    )

  private def validateNanoseconds(nanoseconds: Int): Unit =
    if (nanoseconds < 0 || nanoseconds >= 1000000000)
      throw FromSchemaError(s"datetime nanoseconds out of range: $nanoseconds (expected 0..999999999)")

  private def richBody(reflect: Reflect.Bound[?]): Option[SchemaTypeBody] =
    if (isPath(reflect)) Some(SchemaTypeBody.PathType(GolemPath.defaultSpec))
    else if (isUrl(reflect)) Some(SchemaTypeBody.UrlType(Url.defaultRestrictions))
    else if (isInstant(reflect)) Some(SchemaTypeBody.DatetimeType)
    else if (isDuration(reflect)) Some(SchemaTypeBody.DurationType)
    else quantitySpec(reflect).map(SchemaTypeBody.QuantityType.apply)

  private def unsignedBody(reflect: Reflect.Bound[?]): Option[SchemaTypeBody] = {
    val tid = reflect.typeId
    if (TypeId.structurallyEqual(tid, ubyteTypeId)) Some(SchemaTypeBody.U8Type())
    else if (TypeId.structurallyEqual(tid, ushortTypeId)) Some(SchemaTypeBody.U16Type())
    else if (TypeId.structurallyEqual(tid, uintTypeId)) Some(SchemaTypeBody.U32Type())
    else if (TypeId.structurallyEqual(tid, ulongTypeId)) Some(SchemaTypeBody.U64Type())
    else None
  }

  private def checkUnsigned(value: Long, max: Long, typeName: String, err: String => RuntimeException): Long = {
    if (value < 0L || value > max)
      throw err(s"$typeName value out of range: $value (expected 0..$max)")
    value
  }

  private def checkUnsignedBig(
    value: BigInt,
    max: BigInt,
    typeName: String,
    err: String => RuntimeException
  ): BigInt = {
    if (value < 0 || value > max)
      throw err(s"$typeName value out of range: $value (expected 0..$max)")
    value
  }

  private def isSetTypeId(typeId: TypeId[?]): Boolean =
    TypeId.normalize(typeId).fullName == TypeId.set.fullName

  private def isTupleRecord(reflect: Reflect.Bound[?]): Boolean =
    reflect.typeId.isTuple

  /** Tuple fields, ordered numerically by their `_N` name. */
  private def tupleFieldsInOrder(rec: Reflect.Record.Bound[?]): List[Reflect.Bound[Any]] =
    rec.fields.toList
      .sortBy(f => tupleIndex(f.name))
      .map(_.value.asInstanceOf[Reflect.Bound[Any]])

  private def tupleIndex(name: String): Int =
    try name.drop(1).toInt
    catch { case _: NumberFormatException => Int.MaxValue }

  /**
   * Classify an `Option`-like variant and return the inner element reflect and
   * whether `Some` wraps its payload in a single-field `value` record.
   */
  private def optionInfo(reflect: Reflect.Bound[?]): Option[(Reflect.Bound[Any], Boolean)] =
    if (!reflect.typeId.isOption) None
    else
      reflect.asVariant.flatMap { variant =>
        variant.cases.find(c => simpleCaseName(c.name) == "Some").map { someCase =>
          val someValue = someCase.value.asInstanceOf[Reflect.Bound[Any]]
          someValue.asRecord.flatMap(_.fieldByName("value")) match {
            case Some(valueField) => (valueField.value.asInstanceOf[Reflect.Bound[Any]], true)
            case None             => (someValue, false)
          }
        }
      }

  /**
   * Classify an `Either`-like variant and return the inner Left (err) / Right
   * (ok) element reflects (each `None` when the side carries no value).
   */
  private def eitherInfo(
    reflect: Reflect.Bound[?]
  ): Option[(Option[Reflect.Bound[Any]], Option[Reflect.Bound[Any]])] =
    if (!reflect.typeId.isEither) None
    else
      reflect.asVariant.flatMap { variant =>
        val leftCase  = variant.cases.find(c => simpleCaseName(c.name) == "Left")
        val rightCase = variant.cases.find(c => simpleCaseName(c.name) == "Right")
        if (leftCase.isEmpty || rightCase.isEmpty) None
        else
          Some(
            (
              extractValueRef(leftCase.get.value.asInstanceOf[Reflect.Bound[Any]]),
              extractValueRef(rightCase.get.value.asInstanceOf[Reflect.Bound[Any]])
            )
          )
      }

  private def extractValueRef(caseReflect: Reflect.Bound[Any]): Option[Reflect.Bound[Any]] =
    caseReflect.asRecord match {
      case Some(r) if r.fields.isEmpty                                      => None
      case Some(r) if r.fields.length == 1 && r.fields.head.name == "value" =>
        Some(r.fields.head.value.asInstanceOf[Reflect.Bound[Any]])
      case _ => Some(caseReflect)
    }

  private def simpleCaseName(name: String): String = {
    val afterDot = name.lastIndexOf('.') match {
      case -1 => name
      case i  => name.substring(i + 1)
    }
    if (afterDot.endsWith("$")) afterDot.dropRight(1) else afterDot
  }

  /**
   * Payload shape of a variant case (mirrors [[extractValueRef]] semantics).
   */
  private def casePayloadRef(caseReflect: Reflect.Bound[Any]): Option[Reflect.Bound[Any]] =
    extractValueRef(caseReflect)

  private def uuidRecordBody: SchemaType =
    t.record(List(t.field("high-bits", t.u64), t.field("low-bits", t.u64)))

  // ---------------------------------------------------------------------------
  // Reflect -> SchemaType (graph building)
  // ---------------------------------------------------------------------------

  private def reflectToSchema[A](reflect: Reflect.Bound[A], ctx: Ctx): SchemaType = {
    failIfUnsupported(reflect)

    richBody(reflect) match {
      case Some(body)              => SchemaType(body)
      case None if isUuid(reflect) =>
        ctx.register(UuidTypeId, reflect.typeId, Some(UuidName), () => uuidRecordBody)
      case None =>
        unsignedBody(reflect) match {
          case Some(body) => SchemaType(body)
          case None       =>
            reflect.asWrapperUnknown match {
              case Some(unknown) =>
                reflectToSchema(unknown.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]], ctx)
              case None =>
                optionInfo(reflect) match {
                  case Some((innerRef, _)) => t.option(reflectToSchema(innerRef, ctx))
                  case None                =>
                    eitherInfo(reflect) match {
                      case Some((leftRef, rightRef)) =>
                        t.result(rightRef.map(reflectToSchema(_, ctx)), leftRef.map(reflectToSchema(_, ctx)))
                      case None => reflectToSchemaCore(reflect, ctx)
                    }
                }
            }
        }
    }
  }

  private def reflectToSchemaCore[A](reflect: Reflect.Bound[A], ctx: Ctx): SchemaType =
    reflect.asPrimitive match {
      case Some(p) => primitiveToSchema(p.primitiveType)
      case None    =>
        reflect.asRecord match {
          case Some(rec) =>
            if (isTupleRecord(reflect))
              t.tuple(tupleFieldsInOrder(rec).map(reflectToSchema(_, ctx)))
            else
              ctx.register(
                stableId(reflect.typeId),
                reflect.typeId,
                shortName(reflect),
                () =>
                  t.record(
                    rec.fields.toList.map { field =>
                      NamedFieldType(field.name, reflectToSchema(field.value.asInstanceOf[Reflect.Bound[Any]], ctx))
                    }
                  )
              )

          case None =>
            reflect.asSequenceUnknown match {
              case Some(seqUnknown) =>
                t.list(reflectToSchema(seqUnknown.sequence.element.asInstanceOf[Reflect.Bound[Any]], ctx))

              case None =>
                reflect.asMapUnknown match {
                  case Some(mapUnknown) =>
                    t.map(
                      reflectToSchema(mapUnknown.map.key.asInstanceOf[Reflect.Bound[Any]], ctx),
                      reflectToSchema(mapUnknown.map.value.asInstanceOf[Reflect.Bound[Any]], ctx)
                    )

                  case None =>
                    reflect.asVariant match {
                      case Some(variant) =>
                        ctx.register(
                          stableId(reflect.typeId),
                          reflect.typeId,
                          shortName(reflect),
                          () => {
                            val cases = variant.cases.toList.map { c =>
                              val payload =
                                casePayloadRef(c.value.asInstanceOf[Reflect.Bound[Any]]).map(reflectToSchema(_, ctx))
                              VariantCaseType(c.name, payload)
                            }
                            if (cases.forall(_.payload.isEmpty)) t.`enum`(cases.map(_.name))
                            else t.variant(cases)
                          }
                        )

                      case None =>
                        throw SchemaEncodeError(s"unsupported schema reflect: ${reflect.nodeType}")
                    }
                }
            }
        }
    }

  private def primitiveToSchema(pt: PrimitiveType[?]): SchemaType =
    pt match {
      case PrimitiveType.Unit          => t.tuple(Nil)
      case _: PrimitiveType.String     => t.string
      case _: PrimitiveType.Boolean    => t.bool
      case _: PrimitiveType.Byte       => t.s8
      case _: PrimitiveType.Short      => t.s16
      case _: PrimitiveType.Int        => t.s32
      case _: PrimitiveType.Long       => t.s64
      case _: PrimitiveType.Float      => t.f32
      case _: PrimitiveType.Double     => t.f64
      case _: PrimitiveType.Char       => t.char
      case _: PrimitiveType.BigDecimal => t.string
      case _: PrimitiveType.BigInt     => t.string
      case other                       =>
        throw SchemaEncodeError(s"unsupported primitive: ${other.getClass.getName}")
    }

  // ---------------------------------------------------------------------------
  // DynamicValue -> SchemaValue
  // ---------------------------------------------------------------------------

  private def dynamicToSchemaValue[A](reflect: Reflect.Bound[A], d: DV): SchemaValue = {
    failIfUnsupported(reflect)

    richBody(reflect) match {
      case Some(_)                 => richToSchemaValue(reflect, d)
      case None if isUuid(reflect) => uuidToSchemaValue(d)
      case None                    =>
        unsignedFromDynamic(reflect, d).getOrElse {
          reflect.asWrapperUnknown match {
            case Some(unknown) =>
              dynamicToSchemaValue(unknown.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]], d)
            case None =>
              optionInfo(reflect) match {
                case Some((valueRef, usesRecordWrapper)) => optionToSchemaValue(valueRef, usesRecordWrapper, d)
                case None                                =>
                  eitherInfo(reflect) match {
                    case Some((leftRef, rightRef)) => eitherToSchemaValue(leftRef, rightRef, d)
                    case None                      => dynamicToSchemaValueCore(reflect, d)
                  }
              }
          }
        }
    }
  }

  private def stringFromSingleValueRecord(d: DV, typeName: String): String =
    extractSingleRecordPrimitive[String](d, typeName) { case PrimitiveValue.String(v) => v }

  private def richToSchemaValue[A](reflect: Reflect.Bound[A], d: DV): SchemaValue =
    if (isPath(reflect)) SchemaValue.PathValue(stringFromSingleValueRecord(d, "GolemPath"))
    else if (isUrl(reflect)) SchemaValue.UrlValue(stringFromSingleValueRecord(d, "Url"))
    else if (isInstant(reflect))
      d match {
        case DV.Primitive(PrimitiveValue.String(v)) =>
          val instant = Instant.parse(v)
          SchemaValue.DatetimeValue(Datetime(instant.getEpochSecond, instant.getNano))
        case other => throw SchemaEncodeError(s"Instant expected string dynamic value, got $other")
      }
    else if (isDuration(reflect))
      d match {
        case DV.Primitive(PrimitiveValue.Long(v)) => SchemaValue.DurationValue(v)
        case other                                => throw SchemaEncodeError(s"Duration expected long dynamic value, got $other")
      }
    else {
      val spec = quantitySpec(reflect).getOrElse(throw SchemaEncodeError("missing QuantityUnit for Quantity"))
      d match {
        case DV.Record(fields) =>
          val map      = fields.toMap
          val mantissa = map
            .get("mantissa")
            .collect { case DV.Primitive(PrimitiveValue.Long(v)) => v }
            .getOrElse(throw SchemaEncodeError("Quantity.mantissa expected long"))
          val scale = map
            .get("scale")
            .collect { case DV.Primitive(PrimitiveValue.Int(v)) => v }
            .getOrElse(throw SchemaEncodeError("Quantity.scale expected int"))
          val unit = map
            .get("unit")
            .collect { case DV.Primitive(PrimitiveValue.String(v)) => v }
            .getOrElse(throw SchemaEncodeError("Quantity.unit expected string"))
          if (unit != spec.baseUnit && !spec.allowedSuffixes.contains(unit))
            throw SchemaEncodeError(s"unit '$unit' is not allowed for quantity")
          SchemaValue.QuantityValueNode(QuantityValue(mantissa, scale, unit))
        case other => throw SchemaEncodeError(s"Quantity expected record dynamic value, got $other")
      }
    }

  private def unsignedFromDynamic[A](reflect: Reflect.Bound[A], d: DV): Option[SchemaValue] = {
    val tid = reflect.typeId
    if (TypeId.structurallyEqual(tid, ubyteTypeId))
      Some(
        SchemaValue.U8Value(
          checkUnsigned(
            extractSingleRecordPrimitive[Int](d, "UByte") { case PrimitiveValue.Short(v) => v.toInt }.toLong,
            MaxU8,
            "UByte",
            SchemaEncodeError(_)
          ).toInt
        )
      )
    else if (TypeId.structurallyEqual(tid, ushortTypeId))
      Some(
        SchemaValue.U16Value(
          checkUnsigned(
            extractSingleRecordPrimitive[Int](d, "UShort") { case PrimitiveValue.Int(v) => v }.toLong,
            MaxU16,
            "UShort",
            SchemaEncodeError(_)
          ).toInt
        )
      )
    else if (TypeId.structurallyEqual(tid, uintTypeId))
      Some(
        SchemaValue.U32Value(
          checkUnsigned(
            extractSingleRecordPrimitive[Long](d, "UInt") { case PrimitiveValue.Long(v) => v },
            MaxU32,
            "UInt",
            SchemaEncodeError(_)
          )
        )
      )
    else if (TypeId.structurallyEqual(tid, ulongTypeId))
      Some(
        SchemaValue.U64Value(
          U64.toRawBits(
            checkUnsignedBig(
              extractSingleRecordPrimitive[BigInt](d, "ULong") {
                case PrimitiveValue.BigInt(v)     => v
                case PrimitiveValue.BigDecimal(v) => v.toBigInt
              },
              MaxU64,
              "ULong",
              SchemaEncodeError(_)
            )
          )
        )
      )
    else None
  }

  private def extractSingleRecordPrimitive[T](d: DV, typeName: String)(pf: PartialFunction[PrimitiveValue, T]): T =
    d match {
      case DV.Record(fields) =>
        fields.find(_._1 == "value") match {
          case Some((_, DV.Primitive(pv))) =>
            pf.applyOrElse(
              pv,
              (pv: PrimitiveValue) => throw SchemaEncodeError(s"unexpected primitive for $typeName: $pv")
            )
          case other => throw SchemaEncodeError(s"expected primitive 'value' field for $typeName, got $other")
        }
      case other => throw SchemaEncodeError(s"expected record for $typeName, got $other")
    }

  private def uuidToSchemaValue(d: DV): SchemaValue =
    d match {
      case DV.Record(fields) =>
        def field(name: String): BigInt = {
          val raw = fields.find(_._1 == name).map(_._2) match {
            case Some(DV.Primitive(PrimitiveValue.BigInt(v)))     => v
            case Some(DV.Primitive(PrimitiveValue.BigDecimal(v))) =>
              v.toBigIntExact.getOrElse(
                throw SchemaEncodeError(s"Uuid field '$name' expected integral BigDecimal, got $v")
              )
            case other => throw SchemaEncodeError(s"Uuid field '$name' expected BigInt, got $other")
          }
          checkUnsignedBig(raw, MaxU64, s"Uuid.$name", SchemaEncodeError(_))
        }
        SchemaValue.RecordValue(
          List(
            SchemaValue.U64Value(U64.toRawBits(field("highBits"))),
            SchemaValue.U64Value(U64.toRawBits(field("lowBits")))
          )
        )
      case other => throw SchemaEncodeError(s"Uuid expected record dynamic value, got $other")
    }

  private def optionToSchemaValue(valueRef: Reflect.Bound[Any], usesRecordWrapper: Boolean, d: DV): SchemaValue =
    d match {
      case DV.Variant("None", _)       => SchemaValue.OptionValue(None)
      case DV.Variant("Some", payload) =>
        val inner =
          if (usesRecordWrapper) unwrapValueField(payload, "Option(Some)")
          else payload
        SchemaValue.OptionValue(Some(dynamicToSchemaValue(valueRef, inner)))
      case other => throw SchemaEncodeError(s"Option expected Variant dynamic value, got $other")
    }

  private def eitherToSchemaValue(
    leftRef: Option[Reflect.Bound[Any]],
    rightRef: Option[Reflect.Bound[Any]],
    d: DV
  ): SchemaValue =
    d match {
      case DV.Variant("Left", payload) =>
        SchemaValue.ResultValue(
          SchemaResult.Err(leftRef.map(r => dynamicToSchemaValue(r, unwrapValueField(payload, "Either(Left)"))))
        )
      case DV.Variant("Right", payload) =>
        SchemaValue.ResultValue(
          SchemaResult.Ok(rightRef.map(r => dynamicToSchemaValue(r, unwrapValueField(payload, "Either(Right)"))))
        )
      case other => throw SchemaEncodeError(s"Either expected Variant(Left/Right) dynamic value, got $other")
    }

  private def unwrapValueField(payload: DV, ctx: String): DV =
    payload match {
      case DV.Record(fields) =>
        fields.find(_._1 == "value").map(_._2).getOrElse(throw SchemaEncodeError(s"$ctx payload missing 'value' field"))
      case other => other
    }

  private def dynamicToSchemaValueCore[A](reflect: Reflect.Bound[A], d: DV): SchemaValue =
    reflect.asPrimitive match {
      case Some(_) =>
        d match {
          case DV.Primitive(pv) => primitiveToSchemaValue(pv)
          case other            => throw SchemaEncodeError(s"expected primitive dynamic value, found: $other")
        }

      case None =>
        reflect.asRecord match {
          case Some(rec) =>
            d match {
              case DV.Record(fields) =>
                val map = fields.toMap
                if (isTupleRecord(reflect)) {
                  val ordered = rec.fields.toList
                    .sortBy(f => tupleIndex(f.name))
                    .map { f =>
                      val dv = map.getOrElse(f.name, throw SchemaEncodeError(s"tuple field '${f.name}' missing"))
                      dynamicToSchemaValue(f.value.asInstanceOf[Reflect.Bound[Any]], dv)
                    }
                  SchemaValue.TupleValue(ordered)
                } else {
                  val ordered = rec.fields.toList.map { f =>
                    val dv = map.getOrElse(f.name, throw SchemaEncodeError(s"missing field '${f.name}'"))
                    dynamicToSchemaValue(f.value.asInstanceOf[Reflect.Bound[Any]], dv)
                  }
                  SchemaValue.RecordValue(ordered)
                }
              case other => throw SchemaEncodeError(s"expected record dynamic value, found: $other")
            }

          case None =>
            reflect.asSequenceUnknown match {
              case Some(seqUnknown) =>
                d match {
                  case DV.Sequence(values) =>
                    val elemRef = seqUnknown.sequence.element.asInstanceOf[Reflect.Bound[Any]]
                    SchemaValue.ListValue(values.toList.map(v => dynamicToSchemaValue(elemRef, v)))
                  case other => throw SchemaEncodeError(s"expected sequence dynamic value, found: $other")
                }

              case None =>
                reflect.asMapUnknown match {
                  case Some(mapUnknown) =>
                    d match {
                      case DV.Map(entries) =>
                        val keyRef   = mapUnknown.map.key.asInstanceOf[Reflect.Bound[Any]]
                        val valueRef = mapUnknown.map.value.asInstanceOf[Reflect.Bound[Any]]
                        SchemaValue.MapValue(
                          entries.toList.map { case (k, v) =>
                            SchemaMapEntry(dynamicToSchemaValue(keyRef, k), dynamicToSchemaValue(valueRef, v))
                          }
                        )
                      case other => throw SchemaEncodeError(s"expected map dynamic value, found: $other")
                    }

                  case None =>
                    reflect.asVariant match {
                      case Some(variant) =>
                        d match {
                          case DV.Variant(name, payload) =>
                            val cases    = variant.cases.toList
                            val pureEnum =
                              cases.forall(c => casePayloadRef(c.value.asInstanceOf[Reflect.Bound[Any]]).isEmpty)
                            val caseIndex = cases.indexWhere(_.name == name)
                            if (caseIndex < 0) throw SchemaEncodeError(s"unknown variant case '$name'")
                            val caseTerm   = cases(caseIndex)
                            val payloadRef = casePayloadRef(caseTerm.value.asInstanceOf[Reflect.Bound[Any]])
                            payloadRef match {
                              case None if pureEnum => SchemaValue.EnumValue(caseIndex)
                              case None             => SchemaValue.VariantValue(caseIndex, None)
                              case Some(innerRef)   =>
                                val inner = caseTerm.value.asInstanceOf[Reflect.Bound[Any]].asRecord match {
                                  case Some(r) if r.fields.length == 1 && r.fields.head.name == "value" =>
                                    unwrapValueField(payload, s"variant case '$name'")
                                  case _ => payload
                                }
                                SchemaValue.VariantValue(caseIndex, Some(dynamicToSchemaValue(innerRef, inner)))
                            }
                          case other => throw SchemaEncodeError(s"expected variant dynamic value, found: $other")
                        }
                      case None => throw SchemaEncodeError(s"unsupported schema reflect: ${reflect.nodeType}")
                    }
                }
            }
        }
    }

  private def primitiveToSchemaValue(pv: PrimitiveValue): SchemaValue =
    pv match {
      case PrimitiveValue.Unit          => SchemaValue.TupleValue(Nil)
      case PrimitiveValue.String(v)     => SchemaValue.StringValue(v)
      case PrimitiveValue.Boolean(v)    => SchemaValue.BoolValue(v)
      case PrimitiveValue.Byte(v)       => SchemaValue.S8Value(v)
      case PrimitiveValue.Short(v)      => SchemaValue.S16Value(v)
      case PrimitiveValue.Int(v)        => SchemaValue.S32Value(v)
      case PrimitiveValue.Long(v)       => SchemaValue.S64Value(v)
      case PrimitiveValue.Float(v)      => SchemaValue.F32Value(v)
      case PrimitiveValue.Double(v)     => SchemaValue.F64Value(v)
      case PrimitiveValue.Char(v)       => SchemaValue.CharValue(v.toInt)
      case PrimitiveValue.BigDecimal(v) => SchemaValue.StringValue(v.toString)
      case PrimitiveValue.BigInt(v)     => SchemaValue.StringValue(v.toString)
      case other                        =>
        throw SchemaEncodeError(s"unsupported primitive value: ${other.getClass.getName}")
    }

  // ---------------------------------------------------------------------------
  // SchemaValue -> DynamicValue
  // ---------------------------------------------------------------------------

  private def schemaValueToDynamic[A](reflect: Reflect.Bound[A], value: SchemaValue): DV = {
    failIfUnsupported(reflect)

    richBody(reflect) match {
      case Some(_)                 => richToDynamic(reflect, value)
      case None if isUuid(reflect) => uuidToDynamic(reflect, value)
      case None                    =>
        unsignedToDynamic(reflect, value).getOrElse {
          reflect.asWrapperUnknown match {
            case Some(unknown) =>
              schemaValueToDynamic(unknown.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]], value)
            case None =>
              optionInfo(reflect) match {
                case Some((innerRef, usesRecordWrapper)) => optionToDynamic(innerRef, usesRecordWrapper, value)
                case None                                =>
                  eitherInfo(reflect) match {
                    case Some((leftRef, rightRef)) => eitherToDynamic(leftRef, rightRef, value)
                    case None                      => schemaValueToDynamicCore(reflect, value)
                  }
              }
          }
        }
    }
  }

  private def richToDynamic[A](reflect: Reflect.Bound[A], value: SchemaValue): DV =
    if (isPath(reflect))
      value match {
        case SchemaValue.PathValue(v) => DV.Record(Chunk("value" -> DV.Primitive(PrimitiveValue.String(v))))
        case other                    => throw FromSchemaError(s"expected path value for GolemPath, got $other")
      }
    else if (isUrl(reflect))
      value match {
        case SchemaValue.UrlValue(v) => DV.Record(Chunk("value" -> DV.Primitive(PrimitiveValue.String(v))))
        case other                   => throw FromSchemaError(s"expected url value for Url, got $other")
      }
    else if (isInstant(reflect))
      value match {
        case SchemaValue.DatetimeValue(v) =>
          validateNanoseconds(v.nanoseconds)
          DV.Primitive(PrimitiveValue.String(Instant.ofEpochSecond(v.seconds, v.nanoseconds.toLong).toString))
        case other => throw FromSchemaError(s"expected datetime value for Instant, got $other")
      }
    else if (isDuration(reflect))
      value match {
        case SchemaValue.DurationValue(v) => DV.Primitive(PrimitiveValue.Long(v))
        case other                        => throw FromSchemaError(s"expected duration value for Duration, got $other")
      }
    else {
      val spec =
        try quantitySpec(reflect).get
        catch { case e: SchemaEncodeError => throw FromSchemaError(e.message) }
      value match {
        case SchemaValue.QuantityValueNode(v) if v.unit == spec.baseUnit || spec.allowedSuffixes.contains(v.unit) =>
          DV.Record(
            Chunk(
              "mantissa" -> DV.Primitive(PrimitiveValue.Long(v.mantissa)),
              "scale"    -> DV.Primitive(PrimitiveValue.Int(v.scale)),
              "unit"     -> DV.Primitive(PrimitiveValue.String(v.unit))
            )
          )
        case SchemaValue.QuantityValueNode(v) => throw FromSchemaError(s"unit '${v.unit}' is not allowed for quantity")
        case other                            => throw FromSchemaError(s"expected quantity value for Quantity, got $other")
      }
    }

  private def unsignedToDynamic[A](reflect: Reflect.Bound[A], value: SchemaValue): Option[DV] = {
    val tid = reflect.typeId
    if (TypeId.structurallyEqual(tid, ubyteTypeId))
      value match {
        case SchemaValue.U8Value(v) =>
          val checked = checkUnsigned(v.toLong, MaxU8, "UByte", FromSchemaError(_)).toShort
          Some(DV.Record(Chunk("value" -> DV.Primitive(PrimitiveValue.Short(checked)))))
        case other => throw FromSchemaError(s"expected u8 value for UByte, got $other")
      }
    else if (TypeId.structurallyEqual(tid, ushortTypeId))
      value match {
        case SchemaValue.U16Value(v) =>
          val checked = checkUnsigned(v.toLong, MaxU16, "UShort", FromSchemaError(_)).toInt
          Some(DV.Record(Chunk("value" -> DV.Primitive(PrimitiveValue.Int(checked)))))
        case other => throw FromSchemaError(s"expected u16 value for UShort, got $other")
      }
    else if (TypeId.structurallyEqual(tid, uintTypeId))
      value match {
        case SchemaValue.U32Value(v) =>
          val checked = checkUnsigned(v, MaxU32, "UInt", FromSchemaError(_))
          Some(DV.Record(Chunk("value" -> DV.Primitive(PrimitiveValue.Long(checked)))))
        case other => throw FromSchemaError(s"expected u32 value for UInt, got $other")
      }
    else if (TypeId.structurallyEqual(tid, ulongTypeId))
      value match {
        case SchemaValue.U64Value(v) =>
          Some(DV.Record(Chunk("value" -> DV.Primitive(PrimitiveValue.BigInt(U64.fromRawBits(v))))))
        case other => throw FromSchemaError(s"expected u64 value for ULong, got $other")
      }
    else None
  }

  private def uuidToDynamic[A](reflect: Reflect.Bound[A], value: SchemaValue): DV =
    value match {
      case SchemaValue.RecordValue(SchemaValue.U64Value(hi) :: SchemaValue.U64Value(lo) :: Nil) =>
        val names = reflect.asRecord.map(_.fields.toList.map(_.name)).getOrElse(List("highBits", "lowBits"))
        DV.Record(
          Chunk(
            names.head -> DV.Primitive(PrimitiveValue.BigInt(U64.fromRawBits(hi))),
            names(1)   -> DV.Primitive(PrimitiveValue.BigInt(U64.fromRawBits(lo)))
          )
        )
      case other => throw FromSchemaError(s"expected Uuid record value (two u64), got $other")
    }

  private def optionToDynamic(innerRef: Reflect.Bound[Any], usesRecordWrapper: Boolean, value: SchemaValue): DV =
    value match {
      case SchemaValue.OptionValue(None)    => DV.Variant("None", DV.Record(Chunk.empty))
      case SchemaValue.OptionValue(Some(v)) =>
        val dynInner = schemaValueToDynamic(innerRef, v)
        val payload  = if (usesRecordWrapper) DV.Record(Chunk("value" -> dynInner)) else dynInner
        DV.Variant("Some", payload)
      case other => throw FromSchemaError(s"expected option value for Option, got $other")
    }

  private def eitherToDynamic(
    leftRef: Option[Reflect.Bound[Any]],
    rightRef: Option[Reflect.Bound[Any]],
    value: SchemaValue
  ): DV =
    value match {
      case SchemaValue.ResultValue(SchemaResult.Err(err)) =>
        DV.Variant("Left", sidePayload(leftRef, err, "Either(Left)"))
      case SchemaValue.ResultValue(SchemaResult.Ok(ok)) =>
        DV.Variant("Right", sidePayload(rightRef, ok, "Either(Right)"))
      case other => throw FromSchemaError(s"expected result value for Either, got $other")
    }

  private def sidePayload(ref: Option[Reflect.Bound[Any]], v: Option[SchemaValue], ctx: String): DV =
    (ref, v) match {
      case (Some(innerRef), Some(sv)) => DV.Record(Chunk("value" -> schemaValueToDynamic(innerRef, sv)))
      case (None, None)               => DV.Record(Chunk.empty)
      case (Some(_), None)            => throw FromSchemaError(s"$ctx missing value")
      case (None, Some(_))            => throw FromSchemaError(s"$ctx must not carry a value")
    }

  private def schemaValueToDynamicCore[A](reflect: Reflect.Bound[A], value: SchemaValue): DV =
    reflect.asPrimitive match {
      case Some(p) => primitiveSchemaValueToDynamic(p.primitiveType, value)
      case None    =>
        reflect.asRecord match {
          case Some(rec) =>
            if (isTupleRecord(reflect))
              value match {
                case SchemaValue.TupleValue(values) =>
                  val ordered = rec.fields.toList.sortBy(f => tupleIndex(f.name))
                  if (values.length != ordered.length)
                    throw FromSchemaError(s"tuple arity mismatch: expected ${ordered.length}, got ${values.length}")
                  DV.Record(Chunk.fromIterable(ordered.zip(values).map { case (f, sv) =>
                    f.name -> schemaValueToDynamic(f.value.asInstanceOf[Reflect.Bound[Any]], sv)
                  }))
                case other => throw FromSchemaError(s"expected tuple value for tuple, got $other")
              }
            else
              value match {
                case SchemaValue.RecordValue(values) =>
                  val fields = rec.fields.toList
                  if (values.length != fields.length)
                    throw FromSchemaError(s"record arity mismatch: expected ${fields.length}, got ${values.length}")
                  DV.Record(Chunk.fromIterable(fields.zip(values).map { case (f, sv) =>
                    f.name -> schemaValueToDynamic(f.value.asInstanceOf[Reflect.Bound[Any]], sv)
                  }))
                case other => throw FromSchemaError(s"expected record value for record, got $other")
              }

          case None =>
            reflect.asSequenceUnknown match {
              case Some(seqUnknown) =>
                val elemRef = seqUnknown.sequence.element.asInstanceOf[Reflect.Bound[Any]]
                value match {
                  case SchemaValue.ListValue(values) =>
                    DV.Sequence(Chunk.fromIterable(values.map(v => schemaValueToDynamic(elemRef, v))))
                  case other => throw FromSchemaError(s"expected list value for sequence, got $other")
                }

              case None =>
                reflect.asMapUnknown match {
                  case Some(mapUnknown) =>
                    val keyRef   = mapUnknown.map.key.asInstanceOf[Reflect.Bound[Any]]
                    val valueRef = mapUnknown.map.value.asInstanceOf[Reflect.Bound[Any]]
                    value match {
                      case SchemaValue.MapValue(entries) =>
                        DV.Map(Chunk.fromIterable(entries.map { e =>
                          (schemaValueToDynamic(keyRef, e.key), schemaValueToDynamic(valueRef, e.value))
                        }))
                      case other => throw FromSchemaError(s"expected map value for map, got $other")
                    }

                  case None =>
                    reflect.asVariant match {
                      case Some(variant) =>
                        val cases = variant.cases.toList
                        value match {
                          case SchemaValue.EnumValue(caseIndex) =>
                            DV.Variant(cases(boundsCheck(cases, caseIndex)).name, DV.Record(Chunk.empty))
                          case SchemaValue.VariantValue(caseIndex, payload) =>
                            val caseTerm   = cases(boundsCheck(cases, caseIndex))
                            val payloadDyn = payload match {
                              case None     => DV.Record(Chunk.empty)
                              case Some(pv) =>
                                val payloadRef = caseTerm.value.asInstanceOf[Reflect.Bound[Any]]
                                payloadRef.asRecord match {
                                  case Some(r) if r.fields.length == 1 && r.fields.head.name == "value" =>
                                    DV.Record(
                                      Chunk(
                                        "value" -> schemaValueToDynamic(
                                          r.fields.head.value.asInstanceOf[Reflect.Bound[Any]],
                                          pv
                                        )
                                      )
                                    )
                                  case _ => schemaValueToDynamic(payloadRef, pv)
                                }
                            }
                            DV.Variant(caseTerm.name, payloadDyn)
                          case other => throw FromSchemaError(s"expected variant/enum value for variant, got $other")
                        }
                      case None => throw FromSchemaError(s"unsupported schema reflect: ${reflect.nodeType}")
                    }
                }
            }
        }
    }

  private def boundsCheck(cases: List[?], index: Int): Int =
    if (index < 0 || index >= cases.length)
      throw FromSchemaError(s"variant case index $index out of range (0..${cases.length - 1})")
    else index

  private def primitiveSchemaValueToDynamic(pt: PrimitiveType[?], value: SchemaValue): DV =
    value match {
      case SchemaValue.TupleValue(Nil) =>
        pt match {
          case PrimitiveType.Unit => DV.Primitive(PrimitiveValue.Unit)
          case other              => throw FromSchemaError(s"empty tuple is only valid for Unit, found: ${other.getClass.getName}")
        }
      case SchemaValue.StringValue(v) =>
        pt match {
          case _: PrimitiveType.BigDecimal =>
            DV.Primitive(PrimitiveValue.BigDecimal(BigDecimal(v)))
          case _: PrimitiveType.BigInt =>
            DV.Primitive(PrimitiveValue.BigInt(BigInt(v)))
          case _ => DV.Primitive(PrimitiveValue.String(v))
        }
      case SchemaValue.BoolValue(v) => DV.Primitive(PrimitiveValue.Boolean(v))
      case SchemaValue.CharValue(v) =>
        if (v < Char.MinValue.toInt || v > Char.MaxValue.toInt)
          throw FromSchemaError(s"char value out of Scala Char range: $v")
        DV.Primitive(PrimitiveValue.Char(v.toChar))
      case SchemaValue.S8Value(v)  => DV.Primitive(PrimitiveValue.Byte(v))
      case SchemaValue.S16Value(v) => DV.Primitive(PrimitiveValue.Short(v))
      case SchemaValue.S32Value(v) => DV.Primitive(PrimitiveValue.Int(v))
      case SchemaValue.S64Value(v) => DV.Primitive(PrimitiveValue.Long(v))
      case SchemaValue.F32Value(v) => DV.Primitive(PrimitiveValue.Float(v))
      case SchemaValue.F64Value(v) => DV.Primitive(PrimitiveValue.Double(v))
      case other                   => throw FromSchemaError(s"unsupported primitive schema value: $other")
    }
}

/**
 * Raw-bit `u64` <-> unsigned `BigInt` conversions, centralized per Slice 1
 * guidance.
 */
private[golem] object U64 {

  /** Unsigned `BigInt` in `[0, 2^64)` -> raw two's-complement `Long` bits. */
  def toRawBits(value: BigInt): Long = value.toLong

  /** Raw two's-complement `Long` bits -> unsigned `BigInt` in `[0, 2^64)`. */
  def fromRawBits(bits: Long): BigInt =
    if (bits >= 0) BigInt(bits) else BigInt(bits) + (BigInt(1) << 64)
}
