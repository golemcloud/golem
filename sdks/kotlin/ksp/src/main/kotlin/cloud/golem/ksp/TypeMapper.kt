package cloud.golem.ksp

import com.google.devtools.ksp.symbol.ClassKind
import com.google.devtools.ksp.symbol.KSClassDeclaration
import com.google.devtools.ksp.symbol.KSType
import com.google.devtools.ksp.symbol.Modifier

/**
 * Maps Kotlin types to WIT type strings and back.
 *
 * Each Kotlin primitive maps to a distinct WIT width (Int<->s32, Short<->s16, Byte<->s8,
 * UInt<->u32, ..., Float/Double<->f32/f64), plus String<->string, Boolean<->bool and
 * Unit<->(). `fqnToWit` and `witToFqn` are exact inverses, so every supported primitive
 * survives a Kotlin -> WIT -> Kotlin round-trip (verified by the round-trip consistency test).
 */
object TypeMapper {

    /** Map a resolved KSP type to its WIT type string (derived from its [resolve]d [TypeDesc]). */
    fun toWit(type: KSType): String = resolve(type).toWit()

    private val primitives = mapOf(
        "kotlin.Int" to "s32", "kotlin.Long" to "s64", "kotlin.Short" to "s16", "kotlin.Byte" to "s8",
        "kotlin.UInt" to "u32", "kotlin.ULong" to "u64", "kotlin.UShort" to "u16", "kotlin.UByte" to "u8",
        "kotlin.Float" to "f32", "kotlin.Double" to "f64", "kotlin.Boolean" to "bool", "kotlin.String" to "string",
    )

    /**
     * Resolves a Kotlin [KSType] to a [TypeDesc]. Supports primitives, `Unit`, `T?` (option),
     * `List`, `Map`, `Pair`/`Triple` (tuple), enum classes, sealed classes (variant), data classes
     * (record), `Datetime`, and `Either` (result) -- recursing into type arguments/fields.
     */
    fun resolve(type: KSType): TypeDesc {
        val decl = type.declaration
        val fqn = decl.qualifiedName?.asString() ?: error("Cannot resolve type for WIT mapping: $type")

        // Nullable T? -> option<T> (wraps whatever the non-null form resolves to).
        if (type.isMarkedNullable) {
            return TypeDesc.OptionT(resolve(type.makeNotNullable()))
        }

        primitives[fqn]?.let { return TypeDesc.Prim(it) }
        if (fqn == "kotlin.Unit") return TypeDesc.UnitT

        // List<T> -> list<T>.
        if (fqn == "kotlin.collections.List") {
            val elem = type.arguments.single().type?.resolve()
                ?: error("Cannot resolve List element type of $type")
            return TypeDesc.ListT(resolve(elem))
        }

        // Map<K,V> -> map<K,V>.
        if (fqn == "kotlin.collections.Map") {
            val k = type.arguments[0].type?.resolve() ?: error("Cannot resolve Map key type of $type")
            val v = type.arguments[1].type?.resolve() ?: error("Cannot resolve Map value type of $type")
            return TypeDesc.MapT(resolve(k), resolve(v))
        }

        // Pair<A,B> / Triple<A,B,C> -> tuple<...>.
        if (fqn == "kotlin.Pair" || fqn == "kotlin.Triple") {
            val elems = type.arguments.map {
                resolve(it.type?.resolve() ?: error("Cannot resolve tuple element type of $type"))
            }
            return TypeDesc.TupleT(fqn, elems)
        }

        // cloud.golem.Datetime -> datetime.
        if (fqn == "cloud.golem.Datetime") return TypeDesc.DatetimeT

        // cloud.golem.runtime.Either<L,R> -> result<R,L> (Right = ok, Left = err).
        if (fqn == "cloud.golem.runtime.Either") {
            val l = type.arguments[0].type?.resolve() ?: error("Cannot resolve Either Left type of $type")
            val r = type.arguments[1].type?.resolve() ?: error("Cannot resolve Either Right type of $type")
            return TypeDesc.ResultT(ok = resolve(r), err = resolve(l))
        }

        if (decl is KSClassDeclaration) {
            // enum class -> enum<CASE,...> (entry names in declaration order).
            if (decl.classKind == ClassKind.ENUM_CLASS) {
                val cases = decl.declarations.filterIsInstance<KSClassDeclaration>()
                    .filter { it.classKind == ClassKind.ENUM_ENTRY }
                    .map { it.simpleName.asString() }
                    .toList()
                return TypeDesc.EnumT(fqn, cases)
            }
            // sealed class/interface -> variant<Sub:payloadOrUnit,...>. Each direct subclass is a
            // case: an object subclass has no payload; a subclass with constructor params carries a
            // record of those params. Case order is fixed here and reused for decode/encode.
            if (Modifier.SEALED in decl.modifiers) {
                val cases = decl.getSealedSubclasses().map { sub ->
                    val subFqn = sub.qualifiedName?.asString() ?: error("sealed subclass of $fqn has no qualified name")
                    val hasParams = sub.classKind != ClassKind.OBJECT && (sub.primaryConstructor?.parameters?.isNotEmpty() == true)
                    VariantCase(sub.simpleName.asString(), subFqn, if (hasParams) recordOf(sub, subFqn) else null)
                }.toList()
                require(cases.isNotEmpty()) { "sealed type $fqn has no subclasses" }
                return TypeDesc.VariantT(fqn, cases)
            }
            // data class -> record.
            if (Modifier.DATA in decl.modifiers) return recordOf(decl, fqn)
        }

        error("Unsupported Kotlin type for WIT mapping: $fqn (composite kind not yet supported)")
    }

    /** Builds a `record` TypeDesc from [decl]'s primary constructor params (recursing each). */
    private fun recordOf(decl: KSClassDeclaration, fqn: String): TypeDesc.Record {
        val ctor = decl.primaryConstructor ?: error("$fqn has no primary constructor")
        return TypeDesc.Record(
            fqn,
            ctor.parameters.map { p ->
                Field(p.name?.asString() ?: error("Unnamed field in $fqn"), resolve(p.type.resolve()))
            },
        )
    }

    /** Overload for callers that already have the qualified name (e.g. tests). */
    fun fqnToWit(fqn: String): String = when (fqn) {
        "kotlin.Int" -> "s32"
        "kotlin.Long" -> "s64"
        "kotlin.Short" -> "s16"
        "kotlin.Byte" -> "s8"
        "kotlin.UInt" -> "u32"
        "kotlin.ULong" -> "u64"
        "kotlin.UShort" -> "u16"
        "kotlin.UByte" -> "u8"
        "kotlin.Float" -> "f32"
        "kotlin.Double" -> "f64"
        "kotlin.Boolean" -> "bool"
        "kotlin.String" -> "string"
        "kotlin.Unit" -> "()"
        else -> error("Unsupported Kotlin type for WIT mapping: $fqn")
    }

    /**
     * Reverse: WIT type string -> Kotlin qualified name. The exact inverse of [fqnToWit]
     * over every supported primitive (each WIT width reconstructs its distinct Kotlin type).
     * Used by the round-trip consistency test.
     */
    fun witToFqn(wit: String): String = when (wit) {
        "s32" -> "kotlin.Int"
        "s64" -> "kotlin.Long"
        "s16" -> "kotlin.Short"
        "s8" -> "kotlin.Byte"
        "u32" -> "kotlin.UInt"
        "u64" -> "kotlin.ULong"
        "u16" -> "kotlin.UShort"
        "u8" -> "kotlin.UByte"
        "f32" -> "kotlin.Float"
        "f64" -> "kotlin.Double"
        "bool" -> "kotlin.Boolean"
        "string" -> "kotlin.String"
        "()" -> "kotlin.Unit"
        else -> error("Unsupported WIT type for Kotlin mapping: $wit")
    }
}
