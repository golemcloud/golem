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

import zio.blocks.chunk.Chunk
import zio.blocks.schema.{DynamicValue, PrimitiveValue, Reflect, Schema}
import zio.blocks.typeid.TypeId

import scala.scalajs.js
import scala.scalajs.js.JSConverters._

/**
 * Encode/decode between **plain JS values** (as produced/consumed by the
 * generated bridge manifest) and Scala values using `zio.blocks.schema.Schema`.
 *
 * JS representation conventions:
 *   - **records**: JS objects with fields
 *   - **sequences**: JS arrays
 *   - **options**: `null` for None, inner value for Some
 */
// Used by plugin-generated Scala shims (in package `golem.internal`), so it must be visible
// outside `golem.runtime.autowire` but still not part of the public API surface.
private[golem] object JsPlainSchemaCodec {
  def decode[A](value: js.Any)(implicit schema: Schema[A]): Either[String, A] =
    schema.fromDynamicValue(fromJs(schema.reflect.asInstanceOf[Reflect.Bound[Any]], value)).left.map(_.toString)

  def encode[A](value: A)(implicit schema: Schema[A]): js.Any =
    toJs(schema.reflect.asInstanceOf[Reflect.Bound[Any]], schema.toDynamicValue(value))

  private def fromJs(reflect0: Reflect.Bound[Any], value0: js.Any): DynamicValue = {
    // Wrapper: treat as underlying.
    reflect0.asWrapperUnknown match {
      case Some(w) =>
        return fromJs(w.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]], value0)
      case None => ()
    }

    // Option-like: Variant(None/Some(value)).
    optionInfo(reflect0) match {
      case Some((innerRef, usesRecordWrapper)) =>
        if (js.isUndefined(value0) || value0 == null) {
          // Option(None): payload is ignored by our detection; use empty record.
          return DynamicValue.Variant("None", DynamicValue.Record(Chunk.empty))
        } else {
          val inner   = fromJs(innerRef, value0)
          val payload =
            if (usesRecordWrapper) DynamicValue.Record(Chunk("value" -> inner))
            else inner
          return DynamicValue.Variant("Some", payload)
        }
      case None => ()
    }

    reflect0.asPrimitive match {
      case Some(p) =>
        val tid                = p.primitiveType.typeId
        val pv: PrimitiveValue =
          if (TypeId.structurallyEqual(tid, TypeId.unit)) PrimitiveValue.Unit
          else if (TypeId.structurallyEqual(tid, TypeId.string)) PrimitiveValue.String(value0.asInstanceOf[String])
          else if (TypeId.structurallyEqual(tid, TypeId.boolean)) PrimitiveValue.Boolean(value0.asInstanceOf[Boolean])
          else if (TypeId.structurallyEqual(tid, TypeId.byte)) PrimitiveValue.Byte(value0.asInstanceOf[Double].toByte)
          else if (TypeId.structurallyEqual(tid, TypeId.short))
            PrimitiveValue.Short(value0.asInstanceOf[Double].toShort)
          else if (TypeId.structurallyEqual(tid, TypeId.int)) PrimitiveValue.Int(value0.asInstanceOf[Double].toInt)
          else if (TypeId.structurallyEqual(tid, TypeId.long)) PrimitiveValue.Long(value0.asInstanceOf[Double].toLong)
          else if (TypeId.structurallyEqual(tid, TypeId.float))
            PrimitiveValue.Float(value0.asInstanceOf[Double].toFloat)
          else if (TypeId.structurallyEqual(tid, TypeId.double)) PrimitiveValue.Double(value0.asInstanceOf[Double])
          else throw new IllegalArgumentException(s"Unsupported primitive for JS codec: $tid")
        DynamicValue.Primitive(pv)

      case None =>
        reflect0.asRecord match {
          case Some(rec) =>
            val dyn    = value0.asInstanceOf[js.Dynamic]
            val fields =
              rec.fields.map { f =>
                val fv = dyn.selectDynamic(f.name).asInstanceOf[js.Any]
                f.name -> fromJs(f.value.asInstanceOf[Reflect.Bound[Any]], fv)
              }.toVector
            DynamicValue.Record(Chunk.fromIterable(fields))

          case None =>
            reflect0.asSequenceUnknown match {
              case Some(seq) =>
                val arr   = value0.asInstanceOf[js.Array[js.Any]]
                val elems =
                  arr.toVector.map(v => fromJs(seq.sequence.element.asInstanceOf[Reflect.Bound[Any]], v)).toVector
                DynamicValue.Sequence(Chunk.fromIterable(elems))

              case None =>
                reflect0.asMapUnknown match {
                  case Some(map) =>
                    val obj      = value0.asInstanceOf[js.Dictionary[js.Any]]
                    val keyRef   = map.map.key.asInstanceOf[Reflect.Bound[Any]]
                    val valueRef = map.map.value.asInstanceOf[Reflect.Bound[Any]]
                    val entries  =
                      obj.toVector.map { case (k, v) =>
                        val kd = fromJs(keyRef, k.asInstanceOf[js.Any])
                        val vd = fromJs(valueRef, v)
                        (kd, vd)
                      }.toVector
                    DynamicValue.Map(Chunk.fromIterable(entries))

                  case None =>
                    throw new IllegalArgumentException(s"Unsupported schema reflect for JS codec: ${reflect0.nodeType}")
                }
            }
        }
    }
  }

  private def toJs(reflect0: Reflect.Bound[Any], value: DynamicValue): js.Any = {
    // Wrapper: treat as underlying.
    reflect0.asWrapperUnknown match {
      case Some(w) =>
        return toJs(w.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]], value)
      case None => ()
    }

    // Option-like: Variant(None/Some(value)).
    optionInfo(reflect0) match {
      case Some((innerRef, usesRecordWrapper)) =>
        value match {
          case DynamicValue.Variant("None", _)       => null
          case DynamicValue.Variant("Some", payload) =>
            val inner =
              if (usesRecordWrapper)
                payload match {
                  case DynamicValue.Record(fields) =>
                    fields.find(_._1 == "value").map(_._2).getOrElse(DynamicValue.Record(Chunk.empty))
                  case other => other
                }
              else payload
            return toJs(innerRef, inner)
          case _ =>
            // Best-effort: treat unknown as null
            return null
        }
      case None => ()
    }

    (reflect0.asPrimitive, reflect0.asRecord, reflect0.asSequenceUnknown, reflect0.asMapUnknown) match {
      case (Some(_), _, _, _) =>
        value match {
          case DynamicValue.Primitive(pv) =>
            pv match {
              case PrimitiveValue.Unit          => null
              case PrimitiveValue.String(v)     => v
              case PrimitiveValue.Boolean(v)    => v
              case PrimitiveValue.Byte(v)       => v.toDouble
              case PrimitiveValue.Short(v)      => v.toDouble
              case PrimitiveValue.Int(v)        => v.toDouble
              case PrimitiveValue.Long(v)       => v.toDouble
              case PrimitiveValue.Float(v)      => v.toDouble
              case PrimitiveValue.Double(v)     => v
              case PrimitiveValue.BigInt(v)     => v.toString
              case PrimitiveValue.BigDecimal(v) => v.toString
              case PrimitiveValue.Char(v)       => v.toString
              case PrimitiveValue.UUID(v)       => v.toString
              case other                        => other.toString
            }
          case _ => null
        }

      case (_, Some(rec), _, _) =>
        value match {
          case DynamicValue.Record(fields) =>
            val obj = js.Dictionary.empty[js.Any]
            // Keep schema field order
            val map = fields.toMap
            rec.fields.foreach { f =>
              map.get(f.name).foreach { dv =>
                obj.update(f.name, toJs(f.value.asInstanceOf[Reflect.Bound[Any]], dv))
              }
            }
            obj.asInstanceOf[js.Any]
          case _ => js.Dictionary.empty[js.Any].asInstanceOf[js.Any]
        }

      case (_, _, Some(seq), _) =>
        value match {
          case DynamicValue.Sequence(elements) =>
            elements.map(e => toJs(seq.sequence.element.asInstanceOf[Reflect.Bound[Any]], e)).toJSArray
          case _ => new js.Array[js.Any]()
        }

      case (_, _, _, Some(map)) =>
        value match {
          case DynamicValue.Map(entries) =>
            val dict = js.Dictionary.empty[js.Any]
            entries.foreach { case (k, v) =>
              val keyJs  = toJs(map.map.key.asInstanceOf[Reflect.Bound[Any]], k)
              val keyStr = keyJs.asInstanceOf[String]
              dict.update(keyStr, toJs(map.map.value.asInstanceOf[Reflect.Bound[Any]], v))
            }
            dict.asInstanceOf[js.Any]
          case _ => js.Dictionary.empty[js.Any].asInstanceOf[js.Any]
        }

      case _ =>
        null
    }
  }

  // Ported (lightly) from DataInterop: detect Option-like schema (Variant(None/Some(value))).
  private def optionInfo(reflect: Reflect.Bound[Any]): Option[(Reflect.Bound[Any], Boolean)] =
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
              case Some(valueField) => Some((valueField.value.asInstanceOf[Reflect.Bound[Any]], true))
              case None             => Some((someValue, false))
            }
          case None =>
            Some((someValue, false))
        }
      }
    }
}
