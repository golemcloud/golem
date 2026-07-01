package cloud.golem.ksp

import com.google.devtools.ksp.symbol.KSType

/**
 * Maps Kotlin types to WIT type strings and back.
 *
 * NOTE on round-trip (see OPEN_QUESTIONS A-Q1): Phase A's Kotlin/JS binding maps every WIT
 * integer width (u8/u16/u32/s8/s16/s32) to a single Kotlin `Int`. This forward direction
 * (Kotlin -> WIT) preserves the distinction for types the user actually writes, but a WIT
 * width other than s32 cannot survive a Kotlin -> WIT -> Kotlin round-trip through Phase A.
 * For the counter (Int/String/Unit) the round-trip is exact: Int <-> s32.
 */
object TypeMapper {

    /** Map a resolved KSP type to its WIT type string. */
    fun toWit(type: KSType): String {
        val fqn = type.declaration.qualifiedName?.asString()
            ?: error("Cannot resolve type for WIT mapping: $type")
        return fqnToWit(fqn)
    }

    /** Overload for callers that already have the qualified name (e.g. tests). */
    fun fqnToWit(fqn: String): String = when (fqn) {
        "kotlin.Int"     -> "s32"
        "kotlin.Long"    -> "s64"
        "kotlin.Short"   -> "s16"
        "kotlin.Byte"    -> "s8"
        "kotlin.UInt"    -> "u32"
        "kotlin.ULong"   -> "u64"
        "kotlin.UShort"  -> "u16"
        "kotlin.UByte"   -> "u8"
        "kotlin.Float"   -> "f32"
        "kotlin.Double"  -> "f64"
        "kotlin.Boolean" -> "bool"
        "kotlin.String"  -> "string"
        "kotlin.Unit"    -> "()"
        else -> error("Unsupported Kotlin type for WIT mapping: $fqn")
    }

    /**
     * Reverse: WIT type string -> Kotlin qualified name. Used by the round-trip
     * consistency test (C.8). This is the canonical inverse for the types the
     * counter uses; widths that Phase A collapses to Int are intentionally not
     * reconstructed here (see the class note).
     */
    fun witToFqn(wit: String): String = when (wit) {
        "s32"    -> "kotlin.Int"
        "s64"    -> "kotlin.Long"
        "s16"    -> "kotlin.Short"
        "s8"     -> "kotlin.Byte"
        "u32"    -> "kotlin.UInt"
        "u64"    -> "kotlin.ULong"
        "u16"    -> "kotlin.UShort"
        "u8"     -> "kotlin.UByte"
        "f32"    -> "kotlin.Float"
        "f64"    -> "kotlin.Double"
        "bool"   -> "kotlin.Boolean"
        "string" -> "kotlin.String"
        "()"     -> "kotlin.Unit"
        else -> error("Unsupported WIT type for Kotlin mapping: $wit")
    }
}
