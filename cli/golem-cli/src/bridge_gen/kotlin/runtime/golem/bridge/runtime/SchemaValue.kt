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

package golem.bridge.runtime

/**
 * The schema-native value tree. Composite payloads are positional: records carry no field names, variants and enums carry a case
 * index, unions carry the branch tag. Names come from the schema, not from the value.
 *
 * The cases have the same names and shapes as the Golem Kotlin SDK's `cloud.golem.runtime.SchemaValue`, so generated codecs are the same
 * source for both.
 */
sealed class SchemaValue {
    data class Bool(val v: Boolean) : SchemaValue()

    data class S8(val v: Byte) : SchemaValue()

    data class S16(val v: Short) : SchemaValue()

    data class S32(val v: Int) : SchemaValue()

    data class S64(val v: Long) : SchemaValue()

    data class U8(val v: UByte) : SchemaValue()

    data class U16(val v: UShort) : SchemaValue()

    data class U32(val v: UInt) : SchemaValue()

    data class U64(val v: ULong) : SchemaValue()

    data class F32(val v: Float) : SchemaValue()

    data class F64(val v: Double) : SchemaValue()

    /** A Unicode scalar value in the Basic Multilingual Plane. */
    data class Chr(val v: Char) : SchemaValue()

    data class Str(val v: String) : SchemaValue()

    data class Record(val fields: List<SchemaValue>) : SchemaValue()

    data class ListVal(val items: List<SchemaValue>) : SchemaValue()

    /** A `fixed-list` value: the schema carries the length. */
    data class FixedListVal(val items: List<SchemaValue>) : SchemaValue()

    data class TupleVal(val items: List<SchemaValue>) : SchemaValue()

    data class OptionVal(val inner: SchemaValue?) : SchemaValue()

    data class ResultVal(val ok: Boolean, val inner: SchemaValue?) : SchemaValue()

    data class VariantVal(val caseIndex: Int, val payload: SchemaValue?) : SchemaValue()

    data class EnumVal(val caseIndex: Int) : SchemaValue()

    /** One flag per name declared in the schema, in schema order. */
    data class FlagsVal(val flags: List<Boolean>) : SchemaValue()

    data class MapVal(val entries: List<Pair<SchemaValue, SchemaValue>>) : SchemaValue()

    data class TextVal(val text: String, val language: String?) : SchemaValue()

    data class BinaryVal(val bytes: List<UByte>, val mimeType: String?) : SchemaValue()

    data class PathVal(val v: String) : SchemaValue()

    data class UrlVal(val v: String) : SchemaValue()

    data class UuidVal(val high: Long, val low: Long) : SchemaValue()

    data class DatetimeVal(val seconds: Long, val nanoseconds: Int) : SchemaValue()

    data class DurationVal(val nanoseconds: Long) : SchemaValue()

    data class QuantityVal(val mantissa: Long, val scale: Int, val unit: String) : SchemaValue()

    data class UnionVal(val tag: String, val body: SchemaValue) : SchemaValue()

    /** A public invocation-session stream reference. Exactly one of [provisionalRef] and [streamToken] is non-null. */
    data class StreamRef(val provisionalRef: String?, val streamToken: String?) : SchemaValue()
}
