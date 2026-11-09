package cloud.golem.ksp

import com.google.devtools.ksp.symbol.KSAnnotated
import com.google.devtools.ksp.symbol.KSAnnotation
import com.google.devtools.ksp.symbol.KSClassDeclaration
import com.google.devtools.ksp.symbol.KSFunctionDeclaration
import com.google.devtools.ksp.symbol.KSType
import com.google.devtools.ksp.symbol.KSValueParameter

internal const val ARG_FQN = "cloud.golem.annotations.Arg"
private const val CONSTRAINT_FQN = "cloud.golem.annotations.Constraint"
private const val RESULT_FQN = "cloud.golem.annotations.ToolResult"
private const val EXAMPLE_FQN = "cloud.golem.annotations.Example"
private const val COMMAND_FQN = "cloud.golem.annotations.ToolCommand"
private const val HINTS_FQN = "cloud.golem.annotations.ToolAnnotations"
private const val NO_DEFAULT = "<no-default>"
private const val SCHEMA_VALUE = "cloud.golem.runtime.SchemaValue"

enum class ToolArgKind { POSITIONAL, TAIL, OPTION, FLAG, COUNT_FLAG }

enum class ToolOptionShape { SCALAR, OPTIONAL_SCALAR, REPEATABLE_LIST }

/** One parameter of a command or group method, as the command line sees it. */
data class ToolArgModel(
    val paramName: String,
    /** The rendered Kotlin type of the parameter. */
    val kotlinType: String,
    /** The kebab-case argument name, also the field name in the invocation input record. */
    val name: String,
    val kind: ToolArgKind,
    val shape: ToolOptionShape?,
    /** The declared parameter type; it is also the type of the argument's field in the invocation input record. */
    val typeDesc: TypeDesc,
    /** The type registered in the tool's schema: the value type of a scalar option, the item type of a repeatable option or tail; `null` for a flag. */
    val nodeTypeDesc: TypeDesc?,
    val short: Char?,
    val aliases: List<String>,
    val env: String?,
    /** A Kotlin expression building the default `SchemaValue`, or `null`. */
    val defaultExpr: String?,
    val negatable: Boolean,
    val flagDefault: Boolean,
    /** `REPEATED`, `DELIMITED` or `EITHER` for a repeatable option. */
    val repetition: String?,
    val delimiter: Char?,
    val minItems: Int,
    val maxItems: Int?,
    val separator: String?,
    val verbatim: Boolean,
    val acceptsStdio: Boolean,
    val doc: String,
    val valueName: String?,
    /** Inclusive numeric bounds of the registered type, as the literals the user wrote; `null` is open. */
    val min: String? = null,
    val max: String? = null,
    /** The unit of the registered numeric type; `null` when undeclared. */
    val unit: String? = null,
) {
    /**
     * [desc] as a witType Kotlin expression for registration and for the input record of a call: the plain witType, with this argument's
     * restrictions attached to the number or text it declares (the item of an option, list or tail).
     */
    fun registeredWit(desc: TypeDesc, runtime: String): String {
        if (!restricts(desc)) return kotlinStringLiteral(desc.toWit())
        return when (desc) {
            is TypeDesc.OptionT -> "\"option<\" + ${registeredWit(desc.inner, runtime)} + \">\""
            is TypeDesc.ListT -> "\"list<\" + ${registeredWit(desc.elem, runtime)} + \">\""
            is TypeDesc.TextT -> "$runtime.refinedWit(\"text\", null, null, ${desc.minLength ?: "null"}, ${desc.maxLength ?: "null"}, ${desc.regex?.let { kotlinStringLiteral(it) } ?: "null"})"
            else -> {
                val arguments = listOfNotNull(
                    kotlinStringLiteral(desc.toWit()),
                    min?.let { kotlinStringLiteral(it) } ?: "null",
                    max?.let { kotlinStringLiteral(it) } ?: "null",
                    unit?.let { "unit = ${kotlinStringLiteral(it)}" },
                )
                "$runtime.refinedWit(${arguments.joinToString(", ")})"
            }
        }
    }

    private fun restricts(desc: TypeDesc): Boolean = when (desc) {
        is TypeDesc.OptionT -> restricts(desc.inner)
        is TypeDesc.ListT -> restricts(desc.elem)
        is TypeDesc.TextT -> desc.regex != null || desc.minLength != null || desc.maxLength != null
        is TypeDesc.Prim -> min != null || max != null || unit != null
        else -> false
    }

    val isFlag: Boolean get() = kind == ToolArgKind.FLAG || kind == ToolArgKind.COUNT_FLAG
}

/** A reference in a constraint; [valueExpr] is a Kotlin expression building the `SchemaValue` the argument must have, or `null` for presence. */
data class ToolRefModel(val name: String, val valueExpr: String?)

sealed class ToolConstraintModel {
    data class RequiresAll(val refs: List<ToolRefModel>) : ToolConstraintModel()

    data class RequiresAny(val refs: List<ToolRefModel>) : ToolConstraintModel()

    data class AllOrNone(val refs: List<ToolRefModel>) : ToolConstraintModel()

    data class MutexGroups(val groups: List<List<ToolRefModel>>) : ToolConstraintModel()

    data class Implies(val lhsAny: Boolean, val lhs: List<ToolRefModel>, val rhsAny: Boolean, val rhs: List<ToolRefModel>) : ToolConstraintModel()

    data class Forbids(val lhsAny: Boolean, val lhs: List<ToolRefModel>, val rhs: List<ToolRefModel>) : ToolConstraintModel()
}

data class ToolHintsModel(val readOnly: Boolean, val destructive: Boolean, val idempotent: Boolean, val openWorld: Boolean)

/** The documentation and result metadata read from a command method's annotations. */
data class ToolCommandMetadata(
    val description: String,
    val aliases: List<String>,
    val formatters: List<String>,
    val defaultFormatter: String,
    val examples: List<Pair<String, String>>,
    val hints: ToolHintsModel?,
    val constraints: List<ToolConstraintModel>,
    val stdin: Boolean,
    val stdout: Boolean,
    val stderr: Boolean,
)

/** The argument names a constraint may refer to (by name or alias) and the value type of each, or `null` for a flag. */
private class Scope(val args: List<ToolArgModel>) {
    fun find(name: String): ToolArgModel? = args.firstOrNull { it.name == name || name in it.aliases }
}

private fun KSAnnotation.strings(name: String): List<String> = (arg(name) as? List<*>)?.map { it.toString() } ?: emptyList()

private fun KSAnnotation.annotations(name: String): List<KSAnnotation> = (arg(name) as? List<*>)?.filterIsInstance<KSAnnotation>() ?: emptyList()

private fun KSAnnotation.bool(name: String, default: Boolean = false): Boolean = arg(name) as? Boolean ?: default

private fun KSAnnotation.string(name: String): String = arg(name) as? String ?: ""

private fun enumEntry(value: Any?): String? = when (value) {
    is KSType -> value.declaration.simpleName.asString()
    is KSClassDeclaration -> value.simpleName.asString()
    null -> null
    else -> value.toString().substringAfterLast('.')
}

/** A Kotlin string literal for [s]: quotes, backslashes, `$` and control characters are escaped, so any text is one line of source. */
internal fun kotlinStringLiteral(s: String): String = buildString {
    append('"')
    for (c in s) {
        when {
            c == '\\' -> append("\\\\")
            c == '"' -> append("\\\"")
            c == '$' -> append("\\$")
            c == '\n' -> append("\\n")
            c == '\r' -> append("\\r")
            c == '\t' -> append("\\t")
            c.code < 0x20 || c.code == 0x7f -> append("\\u").append(c.code.toString(16).padStart(4, '0'))
            else -> append(c)
        }
    }
    append('"')
}

/** A Kotlin expression building the `SchemaValue` for [raw] interpreted as [desc], or `null` when [raw] is not a value of that type. */
internal fun schemaValueExpr(desc: TypeDesc, raw: String): String? = when (desc) {
    is TypeDesc.Prim -> when (desc.wit) {
        "bool" -> if (raw == "true" || raw == "false") "$SCHEMA_VALUE.Bool($raw)" else null
        "s8" -> raw.toByteOrNull()?.let { "$SCHEMA_VALUE.S8(${kotlinStringLiteral(it.toString())}.toByte())" }
        "s16" -> raw.toShortOrNull()?.let { "$SCHEMA_VALUE.S16(${kotlinStringLiteral(it.toString())}.toShort())" }
        "s32" -> raw.toIntOrNull()?.let { "$SCHEMA_VALUE.S32(${kotlinStringLiteral(it.toString())}.toInt())" }
        "s64" -> raw.toLongOrNull()?.let { "$SCHEMA_VALUE.S64(${kotlinStringLiteral(it.toString())}.toLong())" }
        "u8" -> raw.toUByteOrNull()?.let { "$SCHEMA_VALUE.U8(${kotlinStringLiteral(it.toString())}.toUByte())" }
        "u16" -> raw.toUShortOrNull()?.let { "$SCHEMA_VALUE.U16(${kotlinStringLiteral(it.toString())}.toUShort())" }
        "u32" -> raw.toUIntOrNull()?.let { "$SCHEMA_VALUE.U32(${kotlinStringLiteral(it.toString())}.toUInt())" }
        "u64" -> raw.toULongOrNull()?.let { "$SCHEMA_VALUE.U64(${kotlinStringLiteral(it.toString())}.toULong())" }
        "f32" -> raw.toFloatOrNull()?.takeIf { it.isFinite() }?.let { "$SCHEMA_VALUE.F32(${kotlinStringLiteral(it.toString())}.toFloat())" }
        "f64" -> raw.toDoubleOrNull()?.takeIf { it.isFinite() }?.let { "$SCHEMA_VALUE.F64(${kotlinStringLiteral(it.toString())}.toDouble())" }
        "char" -> if (raw.length == 1) "$SCHEMA_VALUE.Chr(${kotlinStringLiteral(raw)}[0])" else null
        "string" -> "$SCHEMA_VALUE.Str(${kotlinStringLiteral(raw)})"
        else -> null
    }
    is TypeDesc.TextT -> "$SCHEMA_VALUE.TextVal(${kotlinStringLiteral(raw)}, null)"
    is TypeDesc.EnumT -> desc.cases.indexOf(raw).takeIf { it >= 0 }?.let { "$SCHEMA_VALUE.EnumVal($it)" }
    else -> null
}

/** Reads `@Arg` parameters, constraints and command metadata, and validates them; problems go through [fail] and make the function return `null`. */
internal class ToolArgumentParser(private val fail: (String, KSAnnotated?) -> Nothing?) {
    fun parseArg(
        owner: String,
        p: KSValueParameter,
        pName: String,
        declared: TypeDesc,
        kotlinType: String,
        fn: KSFunctionDeclaration,
        allowedInGroup: Boolean,
    ): ToolArgModel? {
        val annotation = p.annotations.firstOrNull { it.annotationType.resolve().declaration.qualifiedName?.asString() == ARG_FQN }
        val kind = when (val e = enumEntry(annotation?.arg("kind"))) {
            null, "POSITIONAL" -> ToolArgKind.POSITIONAL
            "TAIL" -> ToolArgKind.TAIL
            "OPTION" -> ToolArgKind.OPTION
            "FLAG" -> ToolArgKind.FLAG
            "COUNT_FLAG" -> ToolArgKind.COUNT_FLAG
            else -> return fail("$owner parameter '$pName' has an unknown argument kind '$e'", fn)
        }
        val where = "$owner parameter '$pName'"
        val regexLiteral = annotation?.string("regex")?.takeIf { it.isNotEmpty() }
        val minLengthLiteral = (annotation?.arg("minLength") as? Int)?.takeIf { it >= 0 }
        val maxLengthLiteral = (annotation?.arg("maxLength") as? Int)?.takeIf { it >= 0 }
        val hasTextRestriction = regexLiteral != null || minLengthLiteral != null || maxLengthLiteral != null
        if (annotation?.arg("minLength") as? Int != null && (annotation.arg("minLength") as Int) < -1) return fail("$where: minLength must not be negative", fn)
        if (annotation?.arg("maxLength") as? Int != null && (annotation.arg("maxLength") as Int) < -1) return fail("$where: maxLength must not be negative", fn)
        if (minLengthLiteral != null && maxLengthLiteral != null && minLengthLiteral > maxLengthLiteral) {
            return fail("$where: minLength $minLengthLiteral is above maxLength $maxLengthLiteral", fn)
        }
        val textRestricted = if (hasTextRestriction) {
            restrictText(declared, TypeDesc.TextT(regexLiteral, minLengthLiteral, maxLengthLiteral)) ?: return fail("$where: regex, minLength and maxLength apply only to a String", fn)
        } else {
            declared
        }

        // Path and url refinements narrow a declared GolemPath / Url, as Scala's refinePath and refineUrl
        // narrow a path-typed or url-typed parameter. Neither coerces a String: the declared type decides.
        val direction = annotation?.string("direction")?.takeIf { it.isNotEmpty() }
        val pathKind = annotation?.string("pathKind")?.takeIf { it.isNotEmpty() }
        val mime = (annotation?.arg("mime") as? List<*>)?.map { it.toString() }?.takeIf { it.isNotEmpty() }
        val schemes = (annotation?.arg("schemes") as? List<*>)?.map { it.toString() }?.takeIf { it.isNotEmpty() }
        if (direction != null && direction !in setOf("input", "output", "in-out")) {
            return fail("$where: direction must be \"input\", \"output\" or \"in-out\", found \"$direction\"", fn)
        }
        if (pathKind != null && pathKind !in setOf("file", "directory", "any")) {
            return fail("$where: pathKind must be \"file\", \"directory\" or \"any\", found \"$pathKind\"", fn)
        }
        val desc = when {
            direction != null || pathKind != null || mime != null ->
                narrowPath(textRestricted, direction, pathKind, mime)
                    ?: return fail("$where: direction, pathKind and mime apply only to a GolemPath", fn)
            schemes != null ->
                narrowUrl(textRestricted, schemes)
                    ?: return fail("$where: schemes applies only to a Url", fn)
            else -> textRestricted
        }
        if (allowedInGroup && (kind == ToolArgKind.POSITIONAL || kind == ToolArgKind.TAIL)) {
            return fail("$where of a command group must be an option or flag (annotate it with @Arg(kind = OPTION | FLAG | COUNT_FLAG))", fn)
        }
        val raw = annotation?.arg("default") as? String ?: NO_DEFAULT
        val hasDefault = raw != NO_DEFAULT
        val negatable = annotation?.bool("negatable") ?: false
        val short = (annotation?.arg("short") as? Char)?.takeIf { it != '\u0000' }
        val repeat = enumEntry(annotation?.arg("repeatable")) ?: "NONE"
        val delim = (annotation?.arg("delim") as? Char)?.takeIf { it != '\u0000' }
        val minItems = annotation?.arg("minItems") as? Int ?: 0
        val maxRaw = annotation?.arg("maxItems") as? Int ?: -1
        val maxItems = maxRaw.takeIf { it >= 0 }
        val separator = annotation?.string("separator")?.takeIf { it.isNotEmpty() }
        val verbatim = annotation?.bool("verbatim") ?: false

        val positional = kind == ToolArgKind.POSITIONAL || kind == ToolArgKind.TAIL
        if (positional && !annotation?.string("env").isNullOrEmpty()) return fail("$where: env applies only to an option or a flag", fn)
        if (positional && !annotation?.strings("aliases").isNullOrEmpty()) return fail("$where: aliases apply only to an option or a flag", fn)
        if (!positional && annotation?.bool("acceptsStdio") == true) return fail("$where: acceptsStdio applies only to a positional or a tail", fn)
        if (negatable && kind != ToolArgKind.FLAG) return fail("$where: only a flag can be negatable", fn)
        if (repeat != "NONE" && !(kind == ToolArgKind.OPTION && desc is TypeDesc.ListT)) return fail("$where: only an option of type List can be repeatable", fn)
        if (delim != null && repeat != "DELIMITED" && repeat != "EITHER") return fail("$where: a delimiter needs repeatable = DELIMITED or EITHER", fn)
        if ((repeat == "DELIMITED" || repeat == "EITHER") && delim == null) return fail("$where: repeatable = $repeat needs a delimiter (delim)", fn)
        if ((kind == ToolArgKind.POSITIONAL || kind == ToolArgKind.TAIL) && short != null) return fail("$where: a positional cannot have a short form", fn)
        if ((separator != null || verbatim || minItems != 0) && kind != ToolArgKind.TAIL) return fail("$where: separator, verbatim and minItems apply only to a tail", fn)
        if (maxItems != null && kind != ToolArgKind.TAIL && kind != ToolArgKind.COUNT_FLAG) return fail("$where: maxItems applies only to a tail or a count flag", fn)
        if (verbatim && separator == null) return fail("$where: a verbatim tail needs a separator", fn)
        if (maxItems != null && maxItems < minItems) return fail("$where: maxItems $maxItems is below minItems $minItems", fn)

        var shape: ToolOptionShape? = null
        var node: TypeDesc? = desc
        var defaultExpr: String? = null
        var flagDefault = false
        when (kind) {
            ToolArgKind.POSITIONAL -> if (hasDefault) return fail("$where: a positional cannot have a default", fn)
            ToolArgKind.TAIL -> {
                if (desc !is TypeDesc.ListT) return fail("$where: a tail must be a List", fn)
                if (hasDefault) return fail("$where: a tail cannot have a default", fn)
                node = desc.elem
            }
            ToolArgKind.FLAG -> {
                if (desc != TypeDesc.Prim("bool")) return fail("$where: a flag must be a Boolean", fn)
                if (hasDefault) {
                    flagDefault = raw.toBooleanStrictOrNull() ?: return fail("$where: default '$raw' is not a valid bool", fn)
                }
                node = null
            }
            ToolArgKind.COUNT_FLAG -> {
                if (desc != TypeDesc.Prim("u32")) return fail("$where: a count flag must be a UInt", fn)
                if (hasDefault) return fail("$where: a count flag cannot have a default", fn)
                node = null
            }
            ToolArgKind.OPTION -> when (desc) {
                is TypeDesc.OptionT -> {
                    shape = ToolOptionShape.OPTIONAL_SCALAR
                    node = desc.inner
                    if (hasDefault) return fail("$where: a nullable option cannot have a default", fn)
                }
                is TypeDesc.ListT -> {
                    shape = ToolOptionShape.REPEATABLE_LIST
                    node = desc.elem
                    if (hasDefault) return fail("$where: a repeatable option cannot have a default", fn)
                }
                else -> {
                    shape = ToolOptionShape.SCALAR
                    if (hasDefault) {
                        defaultExpr = schemaValueExpr(desc, raw) ?: return fail("$where: default '$raw' is not a valid ${desc.toWit()}", fn)
                    }
                }
            }
        }
        val numericWit = ((node as? TypeDesc.OptionT)?.inner ?: node).let { (it as? TypeDesc.Prim)?.wit }?.takeIf { numericClass(it) != null }
        val minLiteral = annotation?.string("min")?.takeIf { it.isNotEmpty() }
        val maxLiteral = annotation?.string("max")?.takeIf { it.isNotEmpty() }
        val unitLiteral = annotation?.string("unit")?.takeIf { it.isNotEmpty() }
        if (unitLiteral != null && numericWit == null) return fail("$where: unit applies only to a numeric type", fn)
        if (minLiteral != null || maxLiteral != null) {
            val wit = numericWit ?: return fail("$where: min and max apply only to a numeric type", fn)
            val lowBound = minLiteral?.let { checkedBound("min", it, wit, where, fn) ?: return null }
            val highBound = maxLiteral?.let { checkedBound("max", it, wit, where, fn) ?: return null }
            if (lowBound != null && highBound != null && lowBound > highBound) return fail("$where: min ${lowBound.toPlainString()} is above max ${highBound.toPlainString()}", fn)
            if (hasDefault && kind == ToolArgKind.OPTION) {
                val default = raw.toBigDecimalOrNull()
                if (default != null && lowBound != null && default < lowBound) return fail("$where: default ${default.toPlainString()} is below min ${lowBound.toPlainString()}", fn)
                if (default != null && highBound != null && default > highBound) return fail("$where: default ${default.toPlainString()} is above max ${highBound.toPlainString()}", fn)
            }
        }
        val text = node as? TypeDesc.TextT
        if (text != null && hasDefault && shape == ToolOptionShape.SCALAR) {
            val length = raw.codePointCount(0, raw.length)
            if (text.maxLength != null && length > text.maxLength) return fail("$where: default is longer than maxLength ${text.maxLength}", fn)
            if (text.minLength != null && length < text.minLength) return fail("$where: default is shorter than minLength ${text.minLength}", fn)
            // The host's regex dialect differs from the JVM's; a pattern the JVM cannot read is left for the host to judge.
            val matches = text.regex?.let { pattern -> runCatching { Regex(pattern).containsMatchIn(raw) }.getOrNull() }
            if (matches == false) return fail("$where: default does not match the regex", fn)
        }
        val aliases = annotation?.strings("aliases") ?: emptyList()
        val repetition = if (shape == ToolOptionShape.REPEATABLE_LIST) (if (repeat == "NONE") "REPEATED" else repeat) else null
        return ToolArgModel(
            paramName = pName,
            kotlinType = kotlinType,
            name = toKebabCase(pName),
            kind = kind,
            shape = shape,
            typeDesc = desc,
            nodeTypeDesc = node,
            short = short,
            aliases = aliases,
            env = annotation?.string("env")?.takeIf { it.isNotEmpty() },
            defaultExpr = defaultExpr,
            negatable = negatable,
            flagDefault = flagDefault,
            repetition = repetition,
            delimiter = delim,
            minItems = minItems,
            maxItems = maxItems,
            separator = separator,
            verbatim = verbatim,
            acceptsStdio = annotation?.bool("acceptsStdio") ?: false,
            doc = annotation?.string("doc") ?: "",
            valueName = annotation?.string("valueName")?.takeIf { it.isNotEmpty() },
            min = minLiteral,
            max = maxLiteral,
            unit = unitLiteral,
        )
    }

    /** The value of the bound [literal] for the numeric type [wit], or `null` after reporting that it is not a number of that type. */
    /** [desc] with its `String` item replaced by [text] (through an option or a list), or `null` when it has none. */
    /** [desc]'s `GolemPath` narrowed by the given facets (through an option or a list), or `null` when it holds none. */
    private fun narrowPath(desc: TypeDesc, direction: String?, kind: String?, mime: List<String>?): TypeDesc? = when (desc) {
        is TypeDesc.PathT -> TypeDesc.PathT(direction ?: desc.direction, kind ?: desc.kind, mime ?: desc.mimeTypes, desc.extensions)
        is TypeDesc.OptionT -> narrowPath(desc.inner, direction, kind, mime)?.let { TypeDesc.OptionT(it) }
        is TypeDesc.ListT -> narrowPath(desc.elem, direction, kind, mime)?.let { TypeDesc.ListT(it) }
        else -> null
    }

    /** [desc]'s `Url` narrowed to [schemes] (through an option or a list), or `null` when it holds none. */
    private fun narrowUrl(desc: TypeDesc, schemes: List<String>): TypeDesc? = when (desc) {
        is TypeDesc.UrlT -> TypeDesc.UrlT(schemes, desc.hosts)
        is TypeDesc.OptionT -> narrowUrl(desc.inner, schemes)?.let { TypeDesc.OptionT(it) }
        is TypeDesc.ListT -> narrowUrl(desc.elem, schemes)?.let { TypeDesc.ListT(it) }
        else -> null
    }

    private fun restrictText(desc: TypeDesc, text: TypeDesc.TextT): TypeDesc? = when (desc) {
        is TypeDesc.Prim -> if (desc.wit == "string") text else null
        is TypeDesc.OptionT -> restrictText(desc.inner, text)?.let { TypeDesc.OptionT(it) }
        is TypeDesc.ListT -> restrictText(desc.elem, text)?.let { TypeDesc.ListT(it) }
        else -> null
    }

    private fun checkedBound(label: String, literal: String, wit: String, where: String, fn: KSFunctionDeclaration): java.math.BigDecimal? {
        val isFloat = numericClass(wit) == NumericClass.FLOAT
        // Integer bounds use the plain digit grammar of the runtime's parser; floats accept any finite decimal.
        val grammarOk = isFloat || (literal.matches(Regex("-?[0-9]+")) && !(numericClass(wit) == NumericClass.UNSIGNED && literal == "-0"))
        val value = if (grammarOk) literal.toBigDecimalOrNull() else null
        if (value == null || (isFloat && !(literal.toDoubleOrNull()?.isFinite() ?: false))) {
            fail("$where: $label '$literal' is not a valid $wit", fn)
            return null
        }
        val fits = when {
            wit == "f32" -> literal.toFloat().isFinite()
            isFloat -> true
            else -> value >= intMin(wit) && value <= intMax(wit)
        }
        if (!fits) {
            fail("$where: $label '$literal' does not fit $wit", fn)
            return null
        }
        return value
    }

    /** Checks the shape rules over [args] (one command or group) and [inherited] globals; returns `false` after reporting a problem. */
    fun validateSet(owner: String, args: List<ToolArgModel>, inherited: List<ToolArgModel>, at: KSAnnotated): Boolean {
        val tailIndex = args.indexOfFirst { it.kind == ToolArgKind.TAIL }
        if (tailIndex >= 0 && args.drop(tailIndex + 1).any { it.kind == ToolArgKind.POSITIONAL || it.kind == ToolArgKind.TAIL }) {
            fail("$owner: the tail must be the last parameter", at)
            return false
        }
        if (args.count { it.kind == ToolArgKind.TAIL } > 1) {
            fail("$owner declares more than one tail", at)
            return false
        }
        val names = HashMap<String, String>()
        val shorts = HashMap<Char, String>()
        for (a in args) {
            for (n in listOf(a.name) + a.aliases) {
                names.put(n, a.name)?.let {
                    fail("$owner: name '$n' is declared twice (arguments '$it' and '${a.name}')", at)
                    return false
                }
            }
            a.short?.let { c ->
                shorts.put(c, a.name)?.let {
                    fail("$owner: short form '$c' is used by both '$it' and '${a.name}'", at)
                    return false
                }
            }
        }
        for (g in inherited) {
            for (n in listOf(g.name) + g.aliases) {
                if (n in names) {
                    fail("$owner: name '$n' is also a global of an enclosing command group", at)
                    return false
                }
            }
            g.short?.let { c ->
                if (c in shorts) {
                    fail("$owner: short form '$c' is also a global of an enclosing command group", at)
                    return false
                }
            }
        }
        return true
    }

    fun parseMetadata(owner: String, fn: KSFunctionDeclaration, args: List<ToolArgModel>, globals: List<ToolArgModel>): ToolCommandMetadata? {
        val command = fn.annotations.firstOrNull { it.annotationType.resolve().declaration.qualifiedName?.asString() == COMMAND_FQN }
        val result = fn.annotations.firstOrNull { it.annotationType.resolve().declaration.qualifiedName?.asString() == RESULT_FQN }
        val formatters = result?.strings("formatters") ?: emptyList()
        val defaultFormatter = result?.string("default")?.takeIf { it.isNotEmpty() } ?: formatters.firstOrNull() ?: "default"
        if (formatters.isNotEmpty() && defaultFormatter !in formatters) {
            return fail("$owner: default formatter '$defaultFormatter' is not one of the formatters $formatters", fn)
        }
        if (formatters.size != formatters.toSet().size) return fail("$owner declares a result formatter twice", fn)
        val examples = fn.annotations.filter { it.annotationType.resolve().declaration.qualifiedName?.asString() == EXAMPLE_FQN }
            .map { it.string("title") to it.string("body") }.toList()
        val hints = fn.annotations.firstOrNull { it.annotationType.resolve().declaration.qualifiedName?.asString() == HINTS_FQN }?.let {
            ToolHintsModel(it.bool("readOnly"), it.bool("destructive", true), it.bool("idempotent"), it.bool("openWorld", true))
        }
        val scope = Scope(args + globals)
        val constraints = ArrayList<ToolConstraintModel>()
        for (c in fn.annotations.filter { it.annotationType.resolve().declaration.qualifiedName?.asString() == CONSTRAINT_FQN }) {
            parseConstraint(owner, c, scope, fn, constraints) ?: return null
        }
        return ToolCommandMetadata(
            description = command?.string("description") ?: "",
            aliases = command?.strings("aliases") ?: emptyList(),
            formatters = formatters,
            defaultFormatter = defaultFormatter,
            examples = examples,
            hints = hints,
            constraints = constraints,
            stdin = command?.bool("stdin") ?: false,
            stdout = command?.bool("stdout") ?: false,
            stderr = command?.bool("stderr") ?: false,
        )
    }

    private fun ref(owner: String, text: String, scope: Scope, fn: KSFunctionDeclaration): ToolRefModel? {
        val name = text.substringBefore('=')
        val target = scope.find(name) ?: return fail("$owner: constraint refers to '$name', which is not an argument of $owner", fn)
        if (!text.contains('=')) return ToolRefModel(target.name, null)
        val literal = text.substringAfter('=')
        val node = target.nodeTypeDesc ?: return fail("$owner: constraint compares flag '$name' with a value; a flag can only be present", fn)
        val expr = schemaValueExpr(node, literal) ?: return fail("$owner: constraint on '$name': value '$literal' is not a valid ${node.toWit()}", fn)
        return ToolRefModel(target.name, expr)
    }

    private fun refs(owner: String, texts: List<String>, scope: Scope, fn: KSFunctionDeclaration): List<ToolRefModel>? {
        if (texts.isEmpty()) return fail("$owner: a constraint needs at least one argument", fn)?.let { null }
        return texts.map { ref(owner, it, scope, fn) ?: return null }
    }

    private fun parseConstraint(owner: String, c: KSAnnotation, scope: Scope, fn: KSFunctionDeclaration, out: MutableList<ToolConstraintModel>): Unit? {
        var any = false
        c.strings("requiresAll").takeIf { it.isNotEmpty() }?.let {
            out.add(ToolConstraintModel.RequiresAll(refs(owner, it, scope, fn) ?: return null))
            any = true
        }
        c.strings("requiresAny").takeIf { it.isNotEmpty() }?.let {
            out.add(ToolConstraintModel.RequiresAny(refs(owner, it, scope, fn) ?: return null))
            any = true
        }
        c.strings("allOrNone").takeIf { it.isNotEmpty() }?.let {
            out.add(ToolConstraintModel.AllOrNone(refs(owner, it, scope, fn) ?: return null))
            any = true
        }
        c.annotations("mutexGroups").takeIf { it.isNotEmpty() }?.let { groups ->
            val parsed = groups.map { g ->
                val texts = g.strings("refs")
                if (texts.size < 2) return fail("$owner: a mutex group needs at least two arguments", fn)?.let { null }
                refs(owner, texts, scope, fn) ?: return null
            }
            out.add(ToolConstraintModel.MutexGroups(parsed))
            any = true
        }
        for (i in c.annotations("implies")) {
            out.add(
                ToolConstraintModel.Implies(
                    i.bool("lhsAny"),
                    refs(owner, i.strings("lhs"), scope, fn) ?: return null,
                    i.bool("rhsAny"),
                    refs(owner, i.strings("rhs"), scope, fn) ?: return null,
                ),
            )
            any = true
        }
        for (f in c.annotations("forbids")) {
            out.add(
                ToolConstraintModel.Forbids(
                    f.bool("lhsAny"),
                    refs(owner, f.strings("lhs"), scope, fn) ?: return null,
                    refs(owner, f.strings("rhs"), scope, fn) ?: return null,
                ),
            )
            any = true
        }
        if (!any) return fail("$owner: a @Constraint declares nothing", fn)?.let { null }
        return Unit
    }
}

private enum class NumericClass { SIGNED, UNSIGNED, FLOAT }

private fun numericClass(wit: String): NumericClass? = when (wit) {
    "s8", "s16", "s32", "s64" -> NumericClass.SIGNED
    "u8", "u16", "u32", "u64" -> NumericClass.UNSIGNED
    "f32", "f64" -> NumericClass.FLOAT
    else -> null
}

private fun intMin(wit: String): java.math.BigDecimal = when (wit) {
    "s8" -> java.math.BigDecimal(Byte.MIN_VALUE.toInt())
    "s16" -> java.math.BigDecimal(Short.MIN_VALUE.toInt())
    "s32" -> java.math.BigDecimal(Int.MIN_VALUE)
    "s64" -> java.math.BigDecimal(Long.MIN_VALUE)
    else -> java.math.BigDecimal.ZERO
}

private fun intMax(wit: String): java.math.BigDecimal = when (wit) {
    "s8" -> java.math.BigDecimal(Byte.MAX_VALUE.toInt())
    "s16" -> java.math.BigDecimal(Short.MAX_VALUE.toInt())
    "s32" -> java.math.BigDecimal(Int.MAX_VALUE)
    "s64" -> java.math.BigDecimal(Long.MAX_VALUE)
    "u8" -> java.math.BigDecimal(255)
    "u16" -> java.math.BigDecimal(65535)
    "u32" -> java.math.BigDecimal(4294967295L)
    else -> java.math.BigInteger.ONE.shiftLeft(64).subtract(java.math.BigInteger.ONE).let { java.math.BigDecimal(it) }
}
