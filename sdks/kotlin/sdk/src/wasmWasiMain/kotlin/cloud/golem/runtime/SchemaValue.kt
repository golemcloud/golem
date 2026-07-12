@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.runtime

// The Kotlin data model for the `schema-value-tree` value model (golem:core/types@2.0.0).
// Covers the numeric/bool/char primitives, record/list/option/tuple/result,
// variant/enum/flags/map, the rich nodes text/binary/path/
// url/datetime/duration/quantity/union, plus the original core cases
// (s32, string, record, unit). Case/field NAMES are never carried at the value level
// (only case INDICES / a flat field-order list) -- names live in the schema-graph, which the
// value tree deliberately doesn't redundantly repeat (see the WIT doc comment on
// schema-value-tree).
//
// `secret-value`/`quota-token-handle` (this file's last increment) are `own<resource>` handles
// (capability nodes, not data) rather than a nested schema-value-tree structure -- unlike every
// other case above, no lift/lower recursion is needed: an owned resource handle appearing
// inside memory (not as a direct flattened function argument) is just a plain i32 at that
// offset, per the canonical ABI. This turned out to be MUCH simpler than the resource-handle
// canonical ABI work done for `HostApi.getAgents` (constructor + method dispatch): `secret`/
// `quota-token` (golem:core/types@2.0.0) declare NO methods at all ("An unforgeable handle to
// sensitive material held by the runtime... reveal it only through capability-gated host
// interfaces" -- those interfaces are not built yet), so consuming one here is
// only ever read-the-handle-and-hold-it (or drop it) -- there's no `[method]secret.*` import to
// write. `dropSecret`/`dropQuotaToken` below are exposed for exactly that "hold or drop" case.

@kotlin.wasm.WasmImport("golem:core/types@2.0.0", "[resource-drop]secret")
private external fun hostSecretDrop(handle: Int)

@kotlin.wasm.WasmImport("golem:core/types@2.0.0", "[resource-drop]quota-token")
private external fun hostQuotaTokenDrop(handle: Int)

/** Releases a `secret` handle's guest-side handle-table entry. See [SchemaValue.SecretVal]. */
fun dropSecret(handle: Int) = hostSecretDrop(handle)

/** Releases a `quota-token` handle's guest-side handle-table entry. See [SchemaValue.QuotaTokenVal]. */
fun dropQuotaToken(handle: Int) = hostQuotaTokenDrop(handle)
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

    // NOTE: WIT `char` is a full Unicode scalar value (up to U+10FFFF); Kotlin's Char is a
    // 16-bit UTF-16 code unit (max U+FFFF). Values outside the Basic Multilingual Plane are not
    // representable here -- a known gap, not yet hit by any agent this SDK has (a future
    // increment would need to widen this to Int and expose codePointAt-style accessors).
    data class Chr(val v: Char) : SchemaValue()
    data class Str(val v: String) : SchemaValue()
    data class Record(val fields: List<SchemaValue>) : SchemaValue()
    data class ListVal(val items: List<SchemaValue>) : SchemaValue()
    data class TupleVal(val items: List<SchemaValue>) : SchemaValue()
    data class OptionVal(val inner: SchemaValue?) : SchemaValue()
    data class ResultVal(val ok: Boolean, val inner: SchemaValue?) : SchemaValue()

    /** `payload` is null for a case with no payload (or one that's present-but-unset). */
    data class VariantVal(val caseIndex: Int, val payload: SchemaValue?) : SchemaValue()
    data class EnumVal(val caseIndex: Int) : SchemaValue()

    /** One entry per flag NAME declared in the schema, in schema order -- names aren't repeated here. */
    data class FlagsVal(val flags: List<Boolean>) : SchemaValue()
    data class MapVal(val entries: List<Pair<SchemaValue, SchemaValue>>) : SchemaValue()
    data class TextVal(val text: String, val language: String?) : SchemaValue()

    // `List<UByte>`, not ByteArray -- Kotlin data class equals() uses reference equality for
    // array-typed properties, which would break structural comparison (and this SDK's tests).
    data class BinaryVal(val bytes: List<UByte>, val mimeType: String?) : SchemaValue()
    data class PathVal(val v: String) : SchemaValue()
    data class UrlVal(val v: String) : SchemaValue()
    data class DatetimeVal(val seconds: Long, val nanoseconds: Int) : SchemaValue()
    data class DurationVal(val nanoseconds: Long) : SchemaValue()
    data class QuantityVal(val mantissa: Long, val scale: Int, val unit: String) : SchemaValue()
    data class UnionVal(val tag: String, val body: SchemaValue) : SchemaValue()

    /** An owned `secret` resource handle (an opaque i32 token). Call [dropSecret] when done. */
    data class SecretVal(val handle: Int) : SchemaValue()

    /** An owned `quota-token` resource handle (an opaque i32 token). Call [dropQuotaToken] when done. */
    data class QuotaTokenVal(val handle: Int) : SchemaValue()
    object Unit_ : SchemaValue()
}

// Typed accessors for KSP-generated handler/factory lambdas (the native equivalent of the
// JS-path SDK's extractString/extractInt helpers): extract a raw Kotlin value out of a lifted
// SchemaValue, so generated code reads `input[i].asString()` instead of pattern-matching.
fun SchemaValue.asString(): String = (this as SchemaValue.Str).v
fun SchemaValue.asInt(): Int = (this as SchemaValue.S32).v
fun SchemaValue.asBoolean(): Boolean = (this as SchemaValue.Bool).v
fun SchemaValue.asByte(): Byte = (this as SchemaValue.S8).v
fun SchemaValue.asShort(): Short = (this as SchemaValue.S16).v
fun SchemaValue.asLong(): Long = (this as SchemaValue.S64).v
fun SchemaValue.asUByte(): UByte = (this as SchemaValue.U8).v
fun SchemaValue.asUShort(): UShort = (this as SchemaValue.U16).v
fun SchemaValue.asUInt(): UInt = (this as SchemaValue.U32).v
fun SchemaValue.asULong(): ULong = (this as SchemaValue.U64).v
fun SchemaValue.asFloat(): Float = (this as SchemaValue.F32).v
fun SchemaValue.asDouble(): Double = (this as SchemaValue.F64).v
fun SchemaValue.asChar(): Char = (this as SchemaValue.Chr).v
