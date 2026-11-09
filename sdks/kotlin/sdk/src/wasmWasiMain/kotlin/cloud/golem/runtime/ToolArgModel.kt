@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.buildSchemaValueTree
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.writeEmptyListField
import cloud.golem.wasm.writeListField
import cloud.golem.wasm.writeOptionNone
import cloud.golem.wasm.writeStringField

/** How an option collects repeated occurrences. */
sealed interface NativeRepetition {
    data object Repeated : NativeRepetition

    data class Delimited(val delimiter: Int) : NativeRepetition

    data class Either(val delimiter: Int) : NativeRepetition
}

/** The command-line shape of a [NativeArg]; [Tail] is the variadic positional at the end of a command. */
sealed interface NativeArgShape {
    data object Scalar : NativeArgShape

    data object OptionalScalar : NativeArgShape

    data class RepeatableList(val repetition: NativeRepetition) : NativeArgShape

    data class BoolFlag(val default: Boolean, val negatable: Boolean) : NativeArgShape

    data class CountFlag(val max: Int?) : NativeArgShape

    data class Tail(val min: Int, val max: Int?, val separator: String?, val verbatim: Boolean, val acceptsStdio: Boolean) : NativeArgShape
}

/**
 * An option, flag or tail of a command (positionals are [NativeToolParamSchema]). [nodeWitType] is the type registered in the tool's schema graph:
 * the value type for a scalar, the item type for a repeatable list or tail, `null` for a flag. [default] is interpreted against [nodeWitType].
 */
class NativeArg(
    val name: String,
    val shape: NativeArgShape,
    val nodeWitType: String? = null,
    val doc: String = "",
    val valueName: String? = null,
    val short: Int? = null,
    val aliases: List<String> = emptyList(),
    val env: String? = null,
    val default: SchemaValue? = null,
)

/** A reference in a constraint: the argument is present, or present with [value] (interpreted against its type). */
data class ToolRef(val name: String, val value: SchemaValue? = null)

sealed interface ToolConstraint {
    data class RequiresAll(val refs: List<ToolRef>) : ToolConstraint

    data class RequiresAny(val refs: List<ToolRef>) : ToolConstraint

    data class AllOrNone(val refs: List<ToolRef>) : ToolConstraint

    data class MutexGroups(val groups: List<List<ToolRef>>) : ToolConstraint

    data class Implies(val lhsAny: Boolean, val lhs: List<ToolRef>, val rhsAny: Boolean, val rhs: List<ToolRef>) : ToolConstraint

    data class Forbids(val lhsAny: Boolean, val lhs: List<ToolRef>, val rhs: List<ToolRef>) : ToolConstraint
}

data class ToolExample(val title: String, val body: String)

/** The parts of a command body beyond its positional parameters, result type and errors. */
class NativeBodyExtras(
    val tail: NativeArg? = null,
    val options: List<NativeArg> = emptyList(),
    val flags: List<NativeArg> = emptyList(),
    val constraints: List<ToolConstraint> = emptyList(),
    /** Result formatter names; empty means one formatter named `default`. */
    val formatters: List<String> = emptyList(),
    val defaultFormatter: String = "default",
    val annotations: ToolAnnotationsInfo? = null,
    /** The byte streams the command declares; all optional for the caller. */
    val stdin: Boolean = false,
    val stdout: Boolean = false,
    val stderr: Boolean = false,
) {
    internal fun typeRoots(): List<String> = (listOfNotNull(tail) + options + flags).mapNotNull { it.nodeWitType }

    companion object {
        val NONE = NativeBodyExtras()
    }
}

// option-spec 112 B: long@0 short@8 aliases@16 doc@24 value-name@48 shape@60 default@80 required@96 env-var@100
// flag-spec 72 B: long@0 short@8 aliases@16 doc@24 shape@48 env-var@60
// tail-positional 76 B: name@0 doc@8 value-name@32 item-type@44 min@48 max@52 separator@60 verbatim@72 accepts-stdio@73
// constraint 28 B (tag@0, payload@4); ref 24 B (tag@0, string@4 | value-is-ref: name@4, value tree@12); implies-c 24 B; forbids-c 20 B
internal object ArgLayout {
    const val OPTION_SIZE = 112
    const val FLAG_SIZE = 72
    const val TAIL_OPTION_PAYLOAD = 4
    const val CONSTRAINT_SIZE = 28
    const val REF_SIZE = 24
    const val REF_GROUP_SIZE = 8
    const val EXAMPLE_SIZE = 16
}

internal fun lowerOptionString(base: Int, offset: Int, value: String?) {
    if (value == null) {
        writeOptionNone(base, offset)
    } else {
        storeByte(base + offset, 1)
        writeStringField(base, offset + 4, value)
    }
}

private fun lowerOptionChar(base: Int, offset: Int, value: Int?) {
    if (value == null) {
        writeOptionNone(base, offset)
    } else {
        storeByte(base + offset, 1)
        storeInt(base + offset + 4, value)
    }
}

private fun lowerStrings(base: Int, offset: Int, values: List<String>) {
    if (values.isEmpty()) {
        writeEmptyListField(base, offset)
    } else {
        writeListField(base, offset, values.size, 8, 4) { i, ptr -> writeStringField(ptr, 0, values[i]) }
    }
}

/** Copies the 12-byte `schema-value-tree` of [value] to [dest]. */
internal fun lowerValueTreeInto(dest: Int, value: SchemaValue) {
    val tree = buildSchemaValueTree(value)
    storeInt(dest, loadInt(tree))
    storeInt(dest + 4, loadInt(tree + 4))
    storeInt(dest + 8, loadInt(tree + 8))
}

private fun lowerOptionValueTree(base: Int, offset: Int, value: SchemaValue?) {
    if (value == null) {
        writeOptionNone(base, offset)
    } else {
        storeByte(base + offset, 1)
        lowerValueTreeInto(base + offset + 4, value)
    }
}

private fun lowerRepetitionInto(base: Int, r: NativeRepetition) {
    when (r) {
        NativeRepetition.Repeated -> storeByte(base, 0)
        is NativeRepetition.Delimited -> {
            storeByte(base, 1)
            storeInt(base + 4, r.delimiter)
        }
        is NativeRepetition.Either -> {
            storeByte(base, 2)
            storeInt(base + 4, r.delimiter)
        }
    }
}

internal fun lowerOptionsInto(base: Int, offset: Int, options: List<NativeArg>, typeIndex: Map<String, Int>) {
    if (options.isEmpty()) {
        writeEmptyListField(base, offset)
        return
    }
    writeListField(base, offset, options.size, ArgLayout.OPTION_SIZE, 4) { i, o -> lowerOptionInto(o, options[i], typeIndex) }
}

private fun lowerOptionInto(o: Int, a: NativeArg, typeIndex: Map<String, Int>) {
    writeStringField(o, 0, a.name)
    lowerOptionChar(o, 8, a.short)
    lowerStrings(o, 16, a.aliases)
    lowerDocInto(o, 24, a.doc)
    lowerOptionString(o, 48, a.valueName)
    val type = typeIndex.getValue(requireNotNull(a.nodeWitType) { "option '${a.name}' has no type" })
    val shape = o + 60
    when (val s = a.shape) {
        NativeArgShape.Scalar -> {
            storeByte(shape, 0)
            storeInt(shape + 4, type)
        }
        NativeArgShape.OptionalScalar -> {
            storeByte(shape, 1)
            storeInt(shape + 4, type)
        }
        is NativeArgShape.RepeatableList -> {
            storeByte(shape, 2)
            lowerRepetitionInto(shape + 4, s.repetition)
            storeInt(shape + 12, type)
        }
        else -> error("'${a.name}' is not an option shape")
    }
    lowerOptionValueTree(o, 80, a.default)
    storeByte(o + 96, (if (a.shape == NativeArgShape.Scalar && a.default == null) 1 else 0).toByte())
    lowerOptionString(o, 100, a.env)
}

internal fun lowerFlagsInto(base: Int, offset: Int, flags: List<NativeArg>) {
    if (flags.isEmpty()) {
        writeEmptyListField(base, offset)
        return
    }
    writeListField(base, offset, flags.size, ArgLayout.FLAG_SIZE, 4) { i, f -> lowerFlagInto(f, flags[i]) }
}

private fun lowerFlagInto(f: Int, a: NativeArg) {
    writeStringField(f, 0, a.name)
    lowerOptionChar(f, 8, a.short)
    lowerStrings(f, 16, a.aliases)
    lowerDocInto(f, 24, a.doc)
    val shape = f + 48
    when (val s = a.shape) {
        is NativeArgShape.BoolFlag -> {
            storeByte(shape, 0)
            storeByte(shape + 4, (if (s.default) 1 else 0).toByte())
            storeByte(shape + 5, (if (s.negatable) 1 else 0).toByte())
        }
        is NativeArgShape.CountFlag -> {
            storeByte(shape, 1)
            if (s.max == null) {
                storeByte(shape + 4, 0)
            } else {
                storeByte(shape + 4, 1)
                storeInt(shape + 8, s.max)
            }
        }
        else -> error("'${a.name}' is not a flag shape")
    }
    lowerOptionString(f, 60, a.env)
}

internal fun lowerTailInto(base: Int, offset: Int, tail: NativeArg?, typeIndex: Map<String, Int>) {
    if (tail == null) {
        writeOptionNone(base, offset)
        return
    }
    val s = tail.shape as? NativeArgShape.Tail ?: error("'${tail.name}' is not a tail shape")
    storeByte(base + offset, 1)
    val t = base + offset + ArgLayout.TAIL_OPTION_PAYLOAD
    writeStringField(t, 0, tail.name)
    lowerDocInto(t, 8, tail.doc)
    lowerOptionString(t, 32, tail.valueName)
    storeInt(t + 44, typeIndex.getValue(requireNotNull(tail.nodeWitType) { "tail '${tail.name}' has no type" }))
    storeInt(t + 48, s.min)
    if (s.max == null) {
        writeOptionNone(t, 52)
    } else {
        storeByte(t + 52, 1)
        storeInt(t + 56, s.max)
    }
    lowerOptionString(t, 60, s.separator)
    storeByte(t + 72, (if (s.verbatim) 1 else 0).toByte())
    storeByte(t + 73, (if (s.acceptsStdio) 1 else 0).toByte())
}

private fun lowerRefInto(r: Int, ref: ToolRef) {
    val value = ref.value
    if (value == null) {
        storeByte(r, 0)
        writeStringField(r, 4, ref.name)
    } else {
        storeByte(r, 1)
        writeStringField(r, 4, ref.name)
        lowerValueTreeInto(r + 12, value)
    }
}

private fun lowerRefs(base: Int, offset: Int, refs: List<ToolRef>) {
    if (refs.isEmpty()) {
        writeEmptyListField(base, offset)
    } else {
        writeListField(base, offset, refs.size, ArgLayout.REF_SIZE, 4) { i, p -> lowerRefInto(p, refs[i]) }
    }
}

internal fun lowerConstraintsInto(base: Int, offset: Int, constraints: List<ToolConstraint>) {
    if (constraints.isEmpty()) {
        writeEmptyListField(base, offset)
        return
    }
    writeListField(base, offset, constraints.size, ArgLayout.CONSTRAINT_SIZE, 4) { i, c ->
        when (val k = constraints[i]) {
            is ToolConstraint.RequiresAll -> {
                storeByte(c, 0)
                lowerRefs(c, 4, k.refs)
            }
            is ToolConstraint.AllOrNone -> {
                storeByte(c, 1)
                lowerRefs(c, 4, k.refs)
            }
            is ToolConstraint.RequiresAny -> {
                storeByte(c, 2)
                lowerRefs(c, 4, k.refs)
            }
            is ToolConstraint.MutexGroups -> {
                storeByte(c, 3)
                writeListField(c, 4, k.groups.size, ArgLayout.REF_GROUP_SIZE, 4) { j, g -> lowerRefs(g, 0, k.groups[j]) }
            }
            is ToolConstraint.Implies -> {
                storeByte(c, 4)
                storeByte(c + 4, (if (k.lhsAny) 1 else 0).toByte())
                lowerRefs(c, 8, k.lhs)
                storeByte(c + 16, (if (k.rhsAny) 1 else 0).toByte())
                lowerRefs(c, 20, k.rhs)
            }
            is ToolConstraint.Forbids -> {
                storeByte(c, 5)
                storeByte(c + 4, (if (k.lhsAny) 1 else 0).toByte())
                lowerRefs(c, 8, k.lhs)
                lowerRefs(c, 16, k.rhs)
            }
        }
    }
}

internal fun lowerAnnotationsInto(base: Int, offset: Int, a: ToolAnnotationsInfo?) {
    if (a == null) {
        writeOptionNone(base, offset)
        return
    }
    storeByte(base + offset, 1)
    storeByte(base + offset + 1, (if (a.readOnly) 1 else 0).toByte())
    storeByte(base + offset + 2, (if (a.destructive) 1 else 0).toByte())
    storeByte(base + offset + 3, (if (a.idempotent) 1 else 0).toByte())
    storeByte(base + offset + 4, (if (a.openWorld) 1 else 0).toByte())
}
