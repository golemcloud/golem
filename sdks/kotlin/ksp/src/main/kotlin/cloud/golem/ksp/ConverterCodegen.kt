package cloud.golem.ksp

/**
 * Recursive `SchemaValue` <-> Kotlin converter codegen, shared by the agent-registration emitter
 * ([NativeRegistrationEmitter]) and the RPC typed-client emitter ([RemoteAgentEmitter]). Given a
 * [TypeDesc] and a Kotlin expression string, produces a Kotlin expression string for the other
 * direction. All generated references to `SchemaValue` are short (the emitting file must import
 * `cloud.golem.runtime.SchemaValue`).
 */
object ConverterCodegen {

    /** Recursively decodes a lifted [SchemaValue] expression [sv] into a Kotlin value of type [td]. */
    fun decode(td: TypeDesc, sv: String): String = when (td) {
        is TypeDesc.Prim -> "($sv as SchemaValue.${svVariant(td.wit)}).v"
        is TypeDesc.Record ->
            "${td.kotlinFqn}(" +
                td.fields.mapIndexed { i, f -> decode(f.type, "($sv as SchemaValue.Record).fields[$i]") }.joinToString(", ") +
                ")"
        is TypeDesc.ListT -> "($sv as SchemaValue.ListVal).items.map { ${decode(td.elem, "it")} }"
        is TypeDesc.OptionT -> "($sv as SchemaValue.OptionVal).inner?.let { ${decode(td.inner, "it")} }"
        is TypeDesc.EnumT -> "${td.kotlinFqn}.entries[($sv as SchemaValue.EnumVal).caseIndex]"
        is TypeDesc.VariantT -> {
            // `it` = the VariantVal; branch bodies read it.payload!! (a Record for payload cases).
            val branches = td.cases.mapIndexed { i, c ->
                val body = if (c.payload == null) c.kotlinFqn else decode(c.payload, "it.payload!!")
                "$i -> $body"
            }.joinToString("; ")
            "($sv as SchemaValue.VariantVal).let { when (it.caseIndex) { $branches; else -> error(\"unknown variant case\") } }"
        }
        is TypeDesc.MapT ->
            "($sv as SchemaValue.MapVal).entries.associate { (kSv, vSv) -> ${decode(td.key, "kSv")} to ${decode(td.value, "vSv")} }"
        is TypeDesc.TupleT ->
            "${td.kotlinFqn}(" +
                td.elems.mapIndexed { i, e -> decode(e, "($sv as SchemaValue.TupleVal).items[$i]") }.joinToString(", ") +
                ")"
        // ResultVal -> Either (ok -> Right, err -> Left); `it` = the ResultVal.
        is TypeDesc.ResultT ->
            "($sv as SchemaValue.ResultVal).let { if (it.ok) cloud.golem.runtime.Either.Right(${armDecode(td.ok)}) " +
                "else cloud.golem.runtime.Either.Left(${armDecode(td.err)}) }"
        TypeDesc.DatetimeT -> "($sv as SchemaValue.DatetimeVal).let { cloud.golem.Datetime(it.seconds, it.nanoseconds) }"
        TypeDesc.UnitT -> error("converter codegen: cannot decode a Unit value")
    }

    /** Decodes one arm of a `result` — `Unit` for a unit arm, else the arm's value at `it.inner!!`. */
    private fun armDecode(t: TypeDesc): String = if (t is TypeDesc.UnitT) "Unit" else decode(t, "it.inner!!")

    /** Recursively encodes a Kotlin value expression [k] of type [td] into a [SchemaValue]. */
    fun encode(td: TypeDesc, k: String): String = when (td) {
        is TypeDesc.Prim -> "SchemaValue.${svVariant(td.wit)}($k)"
        is TypeDesc.Record ->
            "SchemaValue.Record(listOf(" +
                td.fields.joinToString(", ") { f -> encode(f.type, "$k.${f.name}") } +
                "))"
        is TypeDesc.ListT -> "SchemaValue.ListVal(($k).map { ${encode(td.elem, "it")} })"
        is TypeDesc.OptionT -> "SchemaValue.OptionVal(($k)?.let { ${encode(td.inner, "it")} })"
        is TypeDesc.EnumT -> "SchemaValue.EnumVal(($k).ordinal)"
        is TypeDesc.VariantT -> {
            // `it` = the sealed value (smart-cast per branch); exhaustive over all subclasses.
            val branches = td.cases.mapIndexed { i, c ->
                val payloadExpr = c.payload?.let { pl -> encode(pl, "it") } ?: "null"
                "is ${c.kotlinFqn} -> SchemaValue.VariantVal($i, $payloadExpr)"
            }.joinToString("; ")
            "($k).let { when (it) { $branches } }"
        }
        is TypeDesc.MapT ->
            "SchemaValue.MapVal(($k).entries.map { (key, value) -> ${encode(td.key, "key")} to ${encode(td.value, "value")} })"
        is TypeDesc.TupleT ->
            "SchemaValue.TupleVal(listOf(" +
                td.elems.mapIndexed { i, e -> encode(e, "($k).component${i + 1}()") }.joinToString(", ") +
                "))"
        // Either -> ResultVal; `it` = the Either (smart-cast per branch, so `it.value` is the arm).
        is TypeDesc.ResultT ->
            "($k).let { when (it) { " +
                "is cloud.golem.runtime.Either.Right -> SchemaValue.ResultVal(true, ${armEncode(td.ok)}); " +
                "is cloud.golem.runtime.Either.Left -> SchemaValue.ResultVal(false, ${armEncode(td.err)}) } }"
        TypeDesc.DatetimeT -> "SchemaValue.DatetimeVal(($k).seconds, ($k).nanoseconds)"
        TypeDesc.UnitT -> "SchemaValue.Unit_"
    }

    /** Encodes one arm of a `result` — `null` for a unit arm, else the arm's value at `it.value`. */
    private fun armEncode(t: TypeDesc): String = if (t is TypeDesc.UnitT) "null" else encode(t, "it.value")

    /** The `SchemaValue` subclass name for a WIT primitive (e.g. s32 -> S32, string -> Str). */
    private fun svVariant(wit: String): String = when (wit) {
        "bool" -> "Bool"
        "s8" -> "S8"
        "s16" -> "S16"
        "s32" -> "S32"
        "s64" -> "S64"
        "u8" -> "U8"
        "u16" -> "U16"
        "u32" -> "U32"
        "u64" -> "U64"
        "f32" -> "F32"
        "f64" -> "F64"
        "char" -> "Chr"
        "string" -> "Str"
        else -> error("converter codegen: no SchemaValue variant for WIT primitive '$wit'")
    }
}
