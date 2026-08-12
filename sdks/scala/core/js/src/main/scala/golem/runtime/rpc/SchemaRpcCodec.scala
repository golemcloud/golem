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

package golem.runtime.rpc

import golem.host.SchemaWireInterop
import golem.host.js.schema.{JsSchemaValueTree, JsTypedAgentConfigValue, JsTypedSchemaValue}
import golem.runtime.autowire.SchemaPayload
import golem.schema._
import golem.schema.wire.SchemaWire
import golem.{UByte, UInt, ULong, UShort, Uuid}

import scala.scalajs.js
import scala.scalajs.js.JSConverters._

/**
 * The `golem:agent/host@2.0.0` RPC-boundary codec: encodes the parameter-list
 * value tree and decodes the optional result tree that the v2 `wasm-rpc` host
 * exchanges (`invoke`/`invoke-and-await` take a `schema-value-tree` and return
 * `option<schema-value-tree>`), plus the `typed-schema-value` carrier used for
 * agent-config values and custom errors.
 *
 * This is the RPC value codec for the `golem:core/types@2.0.0` schema model.
 *
 * Mirrors the TS SDK's RPC client (`clientGeneration.ts`):
 *   - method / constructor arguments are encoded as ONE value tree whose root
 *     is the parameter-list record (here: `IntoSchema[In].toValue`, where the
 *     macro shapes `In` as the record of user-supplied parameters);
 *   - `output-schema = unit` => the host returns `none`; `single` =>
 *     `some(tree)` decoded via `FromSchema[Out]`.
 *
 * The optional result of the host `option<schema-value-tree>` is modelled as a
 * Scala [[Option]] here; the raw `@JSImport` host facade (Slice 4d) bridges it
 * to / from `js.UndefOr` at the actual call boundary, keeping this codec free
 * of `js.|` union plumbing.
 */
private[golem] object SchemaRpcCodec {

  private val MaxU64 = (BigInt(1) << 64) - 1

  def encodeValue(value: SchemaValue): JsSchemaValueTree =
    SchemaWireInterop.valueTreeToJs(SchemaWire.schemaValueToWit(value))

  def decodeValue(tree: JsSchemaValueTree): SchemaValue =
    SchemaWire.schemaValueFromWit(SchemaWireInterop.valueTreeFromJs(tree))

  def encodeUByte(v: UByte): SchemaValue = {
    val x = v.value
    if (x < 0 || x > 255)
      throw new RuntimeException(s"UByte value $x out of range [0, 255]")
    SchemaValue.U8Value(x.toInt)
  }

  def encodeUShort(v: UShort): SchemaValue = {
    val x = v.value
    if (x < 0 || x > 65535)
      throw new RuntimeException(s"UShort value $x out of range [0, 65535]")
    SchemaValue.U16Value(x)
  }

  def encodeUInt(v: UInt): SchemaValue = {
    val x = v.value
    if (x < 0L || x > 4294967295L)
      throw new RuntimeException(s"UInt value $x out of range [0, 4294967295]")
    SchemaValue.U32Value(x)
  }

  def encodeULong(v: ULong): SchemaValue = {
    val x = v.value
    if (x < 0 || x > MaxU64)
      throw new RuntimeException(s"ULong value $x out of range [0, $MaxU64]")
    SchemaValue.U64Value((x & MaxU64).toLong)
  }

  def encodeChar(v: Char): SchemaValue = {
    if (java.lang.Character.isSurrogate(v))
      throw new RuntimeException(f"scala.Char U+${v.toInt}%04X is a surrogate, not a valid Unicode scalar value")
    SchemaValue.CharValue(v.toInt)
  }

  private def mismatch(expected: String, got: SchemaValue): Nothing =
    throw new RuntimeException(s"Expected a $expected value, got $got")

  def asBool(v: SchemaValue): Boolean = v match {
    case SchemaValue.BoolValue(x) => x
    case o                        => mismatch("bool", o)
  }
  def asByte(v: SchemaValue): Byte = v match {
    case SchemaValue.S8Value(x) => x
    case o                      => mismatch("s8", o)
  }
  def asShort(v: SchemaValue): Short = v match {
    case SchemaValue.S16Value(x) => x
    case o                       => mismatch("s16", o)
  }
  def asInt(v: SchemaValue): Int = v match {
    case SchemaValue.S32Value(x) => x
    case o                       => mismatch("s32", o)
  }
  def asLong(v: SchemaValue): Long = v match {
    case SchemaValue.S64Value(x) => x
    case o                       => mismatch("s64", o)
  }
  def asUByte(v: SchemaValue): UByte = v match {
    case SchemaValue.U8Value(x) => UByte(x.toShort)
    case o                      => mismatch("u8", o)
  }
  def asUShort(v: SchemaValue): UShort = v match {
    case SchemaValue.U16Value(x) => UShort(x)
    case o                       => mismatch("u16", o)
  }
  def asUInt(v: SchemaValue): UInt = v match {
    case SchemaValue.U32Value(x) => UInt(x)
    case o                       => mismatch("u32", o)
  }
  def asULong(v: SchemaValue): ULong = v match {
    case SchemaValue.U64Value(x) => ULong(BigInt(x) & MaxU64)
    case o                       => mismatch("u64", o)
  }
  def asFloat(v: SchemaValue): Float = v match {
    case SchemaValue.F32Value(x) => x
    case o                       => mismatch("f32", o)
  }
  def asDouble(v: SchemaValue): Double = v match {
    case SchemaValue.F64Value(x) => x
    case o                       => mismatch("f64", o)
  }
  def asChar(v: SchemaValue): Char = v match {
    case SchemaValue.CharValue(x) if x >= 0 && x <= 0xffff && !java.lang.Character.isSurrogate(x.toChar) =>
      x.toChar
    case SchemaValue.CharValue(x) =>
      throw new RuntimeException(f"Schema char U+$x%04X cannot be represented as a scala.Char")
    case o => mismatch("char", o)
  }
  def asString(v: SchemaValue): String = v match {
    case SchemaValue.StringValue(x) => x
    case o                          => mismatch("string", o)
  }
  def asPath(v: SchemaValue): String = v match {
    case SchemaValue.PathValue(x) => x
    case o                        => mismatch("path", o)
  }
  def asUrl(v: SchemaValue): String = v match {
    case SchemaValue.UrlValue(x) => x
    case o                       => mismatch("url", o)
  }
  def asDatetime(v: SchemaValue): java.time.Instant = v match {
    case SchemaValue.DatetimeValue(x) => java.time.Instant.ofEpochSecond(x.seconds, x.nanoseconds.toLong)
    case o                            => mismatch("datetime", o)
  }
  def asDuration(v: SchemaValue): Long = v match {
    case SchemaValue.DurationValue(x) => x
    case o                            => mismatch("duration", o)
  }

  def recordFields(v: SchemaValue): List[SchemaValue] = v match {
    case SchemaValue.RecordValue(fields) => fields
    case o                               => mismatch("record", o)
  }
  def variantCase(v: SchemaValue): (Int, Option[SchemaValue]) = v match {
    case SchemaValue.VariantValue(caseIndex, payload) => (caseIndex, payload)
    case o                                            => mismatch("variant", o)
  }
  def enumCase(v: SchemaValue): Int = v match {
    case SchemaValue.EnumValue(caseIndex) => caseIndex
    case o                                => mismatch("enum", o)
  }
  def flagBits(v: SchemaValue): List[Boolean] = v match {
    case SchemaValue.FlagsValue(bits) => bits
    case o                            => mismatch("flags", o)
  }
  def tupleElements(v: SchemaValue): List[SchemaValue] = v match {
    case SchemaValue.TupleValue(elements) => elements
    case o                                => mismatch("tuple", o)
  }
  def listElements(v: SchemaValue): List[SchemaValue] = v match {
    case SchemaValue.ListValue(elements) => elements
    case o                               => mismatch("list", o)
  }
  def fixedListElements(v: SchemaValue): List[SchemaValue] = v match {
    case SchemaValue.FixedListValue(elements) => elements
    case o                                    => mismatch("fixed-list", o)
  }
  def mapEntries(v: SchemaValue): List[SchemaMapEntry] = v match {
    case SchemaValue.MapValue(entries) => entries
    case o                             => mismatch("map", o)
  }
  def optionValue(v: SchemaValue): Option[SchemaValue] = v match {
    case SchemaValue.OptionValue(inner) => inner
    case o                              => mismatch("option", o)
  }
  def resultValue(v: SchemaValue): SchemaResult = v match {
    case SchemaValue.ResultValue(result) => result
    case o                               => mismatch("result", o)
  }
  def unionBody(v: SchemaValue): (String, SchemaValue) = v match {
    case SchemaValue.UnionValue(unionTag, body) => (unionTag, body)
    case o                                      => mismatch("union", o)
  }

  def requiredPayload(payload: Option[SchemaValue], context: String): SchemaValue =
    payload.getOrElse(throw new RuntimeException(s"Missing payload for $context"))

  def encodeUuid(uuid: Uuid): SchemaValue = {
    if (uuid.highBits < 0 || uuid.highBits > MaxU64)
      throw new RuntimeException(s"UUID high half ${uuid.highBits} out of range [0, $MaxU64]")
    if (uuid.lowBits < 0 || uuid.lowBits > MaxU64)
      throw new RuntimeException(s"UUID low half ${uuid.lowBits} out of range [0, $MaxU64]")
    SchemaValue.RecordValue(
      List(SchemaValue.U64Value((uuid.highBits & MaxU64).toLong), SchemaValue.U64Value((uuid.lowBits & MaxU64).toLong))
    )
  }

  def decodeUuidOrThrow(value: SchemaValue): Uuid = value match {
    case SchemaValue.RecordValue(List(SchemaValue.U64Value(hi), SchemaValue.U64Value(lo))) =>
      Uuid(BigInt(hi) & MaxU64, BigInt(lo) & MaxU64)
    case other => throw new RuntimeException(s"Expected a uuid record (two u64 fields), got $other")
  }

  // --- arguments (constructor + method input) -------------------------------

  /** Encode the parameter-list value of `In` into a `schema-value-tree`. */
  def encodeArgs[In](input: In)(implicit into: IntoSchema[In]): JsSchemaValueTree =
    SchemaPayload.encode(input)

  /** Decode a parameter-list `schema-value-tree` back into `In`. */
  def decodeArgs[In](tree: JsSchemaValueTree)(implicit from: FromSchema[In]): Either[String, In] =
    SchemaPayload.decode[In](tree).left.map(_.toString)

  // --- results (option<schema-value-tree>) ----------------------------------

  /** `output-schema = unit` => no value tree on the wire. */
  val encodeUnitResult: Option[JsSchemaValueTree] = None

  /** `output-schema = single` => `some(tree)`. */
  def encodeSingleResult[Out](value: Out)(implicit into: IntoSchema[Out]): Option[JsSchemaValueTree] =
    Some(SchemaPayload.encode(value))

  /**
   * Decode a unit result. The host returns `none`; a stray `some` is tolerated
   * and ignored (TS parity), so the only outcome is `()`.
   */
  def decodeUnitResult(result: Option[JsSchemaValueTree]): Either[String, Unit] =
    Right(())

  /**
   * Decode a single-value result; absence is an error for a non-unit method.
   */
  def decodeSingleResult[Out](
    result: Option[JsSchemaValueTree]
  )(implicit from: FromSchema[Out]): Either[String, Out] =
    result match {
      case Some(tree) => SchemaPayload.decode[Out](tree).left.map(_.toString)
      case None       => Left("Expected a return value for a non-unit method output, got none")
    }

  // --- typed-schema-value (agent-config values, custom errors) --------------

  /**
   * Encode `value` into a self-contained `typed-schema-value` (graph + value).
   */
  def encodeTyped[A](value: A)(implicit into: IntoSchema[A]): JsTypedSchemaValue =
    SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(into.toTyped(value)))

  /** Decode a `typed-schema-value` back into `A` using `FromSchema[A]`. */
  def decodeTyped[A](typed: JsTypedSchemaValue)(implicit from: FromSchema[A]): Either[String, A] =
    from
      .fromValue(SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(typed)).value)
      .left
      .map(_.toString)

  /**
   * Build a `typed-agent-config-value` (`path` + `typed-schema-value`) entry.
   */
  def typedConfigValue[A](path: List[String], value: A)(implicit into: IntoSchema[A]): JsTypedAgentConfigValue =
    JsTypedAgentConfigValue(path.toJSArray, encodeTyped(value))
}
