@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.liftSingleValue
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt

/** What kind of command-line argument a [ToolArgumentInfo] describes. */
enum class ToolArgumentKind { POSITIONAL, TAIL, OPTION, FLAG }

/** The command-line shape of a [ToolArgumentInfo]. */
enum class ToolArgumentShape { FIXED, TAIL, SCALAR, OPTIONAL_SCALAR, REPEATABLE_LIST, REPEATABLE_MAP, BOOL_FLAG, COUNT_FLAG }

/**
 * One declared argument of a command. [typeIndex] is the argument's node in the tool's schema graph (`null` for flags, whose
 * type is fixed by their shape).
 */
data class ToolArgumentInfo(
    val kind: ToolArgumentKind,
    val name: String,
    val aliases: List<String>,
    /** The short option character as a Unicode scalar, if any. */
    val short: Int?,
    val required: Boolean,
    val summary: String,
    val typeIndex: Int?,
    /**
     * The argument's type as a witType string (the grammar [AgentParameterInfo.witType] uses), resolved through the tool's
     * schema graph; `null` for flags, or when the type cannot be written flat (a recursive type).
     */
    val witType: String? = null,
    val shape: ToolArgumentShape = ToolArgumentShape.FIXED,
    /** The default value, interpreted against [witType]. */
    val default: SchemaValue? = null,
    val env: String? = null,
    val valueName: String? = null,
    /** Bool flags only: whether `--no-<name>` exists, and the value when the flag is absent. */
    val negatable: Boolean = false,
    val flagDefault: Boolean = false,
    /** Count flags only: the maximum count, if any. */
    val countMax: Int? = null,
    /** Tails only. */
    val tailMin: Int = 0,
    val tailMax: Int? = null,
    val separator: String? = null,
    /** The inclusive numeric bounds of the argument's type, as the host reports them (integers in plain digits, floats as `Double.toString()`); `null` when open or when the type is not numeric. */
    val min: String? = null,
    val max: String? = null,
    /** The unit of the argument's numeric type, such as `"ms"`; schema and help metadata, not a constraint. `null` when undeclared or when the type is not numeric. */
    val unit: String? = null,
    /** The text restrictions of the argument's type, when it is `text`: lengths in Unicode scalar values, and a regular expression. */
    val minLength: Long? = null,
    val maxLength: Long? = null,
    val regex: String? = null,
)

/** A command's stream and result contract, read from its body. */
data class ToolBodyInfo(
    val arguments: List<ToolArgumentInfo>,
    val stdin: Boolean,
    val stdout: Boolean,
    val stderr: Boolean,
    /** True when the command declares a structured result. */
    val hasResult: Boolean,
    /** Names of the declared error cases. */
    val errors: List<String>,
    /** The (read-only, destructive, idempotent, open-world) hints, if declared. */
    val annotations: ToolAnnotationsInfo?,
    val constraints: List<ToolConstraint> = emptyList(),
    /** Result formatter names (empty without a result) and the default one. */
    val formatters: List<String> = emptyList(),
    val defaultFormatter: String? = null,
)

data class ToolAnnotationsInfo(val readOnly: Boolean, val destructive: Boolean, val idempotent: Boolean, val openWorld: Boolean)

/** One node of a tool's command tree. [globals] apply to this command and everything below it. */
data class ToolCommandInfo(
    val name: String,
    val aliases: List<String>,
    val summary: String,
    /** Indices into [ToolInfo.commands]. */
    val subcommands: List<Int>,
    val globals: List<ToolArgumentInfo>,
    /** `null` for a command that only groups subcommands. */
    val body: ToolBodyInfo?,
    val examples: List<ToolExample> = emptyList(),
)

/**
 * A registered tool's declared interface: its command tree, decoded from the host's `tool` record. Node 0 is the root and
 * names the tool. Types stay as [ToolArgumentInfo.typeIndex] into the tool's schema graph.
 */
data class ToolInfo(
    val registration: RegisteredTool,
    val requiresFilesystem: Boolean,
    val commands: List<ToolCommandInfo>,
) {
    val root: ToolCommandInfo get() = commands[0]

    /** Resolves a command path (the names below the root, aliases accepted) to its node, or `null`. */
    fun find(path: List<String>): ToolCommandInfo? {
        var node = root
        for (step in path) {
            node = node.subcommands.map { commands[it] }.firstOrNull { step == it.name || step in it.aliases } ?: return null
        }
        return node
    }

    /** Every runnable command path (the commands with a body), as names below the root. */
    fun runnablePaths(): List<List<String>> {
        val out = ArrayList<List<String>>()

        fun walk(node: ToolCommandInfo, path: List<String>) {
            if (node.body != null) out.add(path)
            for (i in node.subcommands) walk(commands[i], path + commands[i].name)
        }
        walk(root, emptyList())
        return out
    }
}

// Layouts from `wasm-tools component wit --json` of wit-native/deps/golem-tool/common.wit (see ToolModel.kt).
private const val NODE_SIZE = 364
private const val OPTION_SIZE = 112
private const val FLAG_SIZE = 72
private const val POSITIONAL_SIZE = 68
private const val ERROR_SIZE = 44

private fun isSome(base: Int): Boolean = loadByte(base).toInt() and 0xFF != 0

private fun str(base: Int): String = liftString(loadInt(base), loadInt(base + 4))

private fun strings(base: Int): List<String> {
    val ptr = loadInt(base)
    return (0 until loadInt(base + 4)).map { str(ptr + it * 8) }
}

private fun summary(doc: Int): String = str(doc)

private fun <T> records(listAt: Int, size: Int, read: (Int) -> T): List<T> {
    val ptr = loadInt(listAt)
    return (0 until loadInt(listAt + 4)).map { read(ptr + it * size) }
}

private fun optionalChar(base: Int): Int? = if (isSome(base)) loadInt(base + 4) else null

private fun optionalString(base: Int): String? = if (isSome(base)) str(base + 4) else null

private fun liftDefault(base: Int, witType: String?): SchemaValue? = if (witType != null && isSome(base)) liftSingleValue(base + 4, witType) else null

private fun ToolArgumentInfo.bounded(types: TypeResolver): ToolArgumentInfo = types.bounds(typeIndex)
    ?.let { copy(min = it.min, max = it.max, minLength = it.minLength, maxLength = it.maxLength, regex = it.regex, unit = it.unit) }
    ?: this

private fun optionArgument(o: Int, types: TypeResolver): ToolArgumentInfo = optionArgumentUnbounded(o, types).bounded(types)

private fun optionArgumentUnbounded(o: Int, types: TypeResolver): ToolArgumentInfo {
    val shape = o + 60
    val tag = loadByte(shape).toInt() and 0xFF
    // scalar / optional-scalar carry the type node itself; the repeatable shapes carry a record that starts with a repetition
    // variant (8 B), so the item / map type sits at +8 of the payload.
    val type = if (tag <= 1) loadInt(shape + 4) else loadInt(shape + 4 + 8)
    val witType = types.witType(type)
    return ToolArgumentInfo(
        ToolArgumentKind.OPTION,
        str(o),
        strings(o + 16),
        optionalChar(o + 8),
        loadByte(o + 96).toInt() != 0,
        summary(o + 24),
        type,
        witType,
        ToolArgumentShape.entries[2 + tag],
        liftDefault(o + 80, witType),
        optionalString(o + 100),
        optionalString(o + 48),
    )
}

/** Resolves type-node indices of one tool's schema graph to witType strings. */
internal class TypeResolver(private val nodesPtr: Int, private val defsPtr: Int) {
    /** The numeric bounds of type node [index]. */
    fun bounds(index: Int?): NumericBounds? = index?.let { liftRefinement(nodesPtr + it * Layout.SCHEMA_TYPE_NODE_SIZE + Layout.STN_BODY) }

    fun witType(index: Int?): String? {
        if (index == null) return null
        return try {
            schemaNodeToWitType(nodesPtr, defsPtr, index)
        } catch (_: IllegalStateException) {
            null
        }
    }
}

private fun flagArgument(f: Int): ToolArgumentInfo {
    val shape = f + 48
    val count = loadByte(shape).toInt() and 0xFF == 1
    return ToolArgumentInfo(
        ToolArgumentKind.FLAG,
        str(f),
        strings(f + 16),
        optionalChar(f + 8),
        false,
        summary(f + 24),
        null,
        shape = if (count) ToolArgumentShape.COUNT_FLAG else ToolArgumentShape.BOOL_FLAG,
        env = optionalString(f + 60),
        negatable = !count && loadByte(shape + 5).toInt() != 0,
        flagDefault = !count && loadByte(shape + 4).toInt() != 0,
        countMax = if (count && isSome(shape + 4)) loadInt(shape + 8) else null,
    )
}

private fun liftRef(r: Int, typeOf: (String) -> String?): ToolRef {
    val name = str(r + 4)
    if (loadByte(r).toInt() == 0) return ToolRef(name)
    val witType = typeOf(name) ?: error("constraint value-is names '$name', which has no declared type")
    return ToolRef(name, liftSingleValue(r + 12, witType))
}

private fun liftRefs(listAt: Int, typeOf: (String) -> String?): List<ToolRef> = records(listAt, ArgLayout.REF_SIZE) { liftRef(it, typeOf) }

private fun liftConstraint(c: Int, typeOf: (String) -> String?): ToolConstraint = when (val tag = loadByte(c).toInt() and 0xFF) {
    0 -> ToolConstraint.RequiresAll(liftRefs(c + 4, typeOf))
    1 -> ToolConstraint.AllOrNone(liftRefs(c + 4, typeOf))
    2 -> ToolConstraint.RequiresAny(liftRefs(c + 4, typeOf))
    3 -> ToolConstraint.MutexGroups(records(c + 4, ArgLayout.REF_GROUP_SIZE) { liftRefs(it, typeOf) })
    4 -> ToolConstraint.Implies(loadByte(c + 4).toInt() != 0, liftRefs(c + 8, typeOf), loadByte(c + 16).toInt() != 0, liftRefs(c + 20, typeOf))
    5 -> ToolConstraint.Forbids(loadByte(c + 4).toInt() != 0, liftRefs(c + 8, typeOf), liftRefs(c + 16, typeOf))
    else -> error("unknown constraint tag $tag")
}

private fun liftBody(body: Int, types: TypeResolver, globalTypes: Map<String, String>): ToolBodyInfo {
    val args = ArrayList<ToolArgumentInfo>()
    records(body, POSITIONAL_SIZE) { p ->
        ToolArgumentInfo(
            ToolArgumentKind.POSITIONAL,
            str(p),
            emptyList(),
            null,
            loadByte(p + 64).toInt() != 0,
            summary(p + 8),
            loadInt(p + 44),
            types.witType(loadInt(p + 44)),
            default = liftDefault(p + 48, types.witType(loadInt(p + 44))),
            valueName = optionalString(p + 32),
        ).bounded(types)
    }.forEach { args.add(it) }
    val tail = body + 8 // option<tail-positional>: tag@0, payload@4
    if (isSome(tail)) {
        val t = tail + 4
        args.add(
            ToolArgumentInfo(
                ToolArgumentKind.TAIL,
                str(t),
                emptyList(),
                null,
                loadInt(t + 48) > 0,
                summary(t + 8),
                loadInt(t + 44),
                types.witType(loadInt(t + 44)),
                ToolArgumentShape.TAIL,
                valueName = optionalString(t + 32),
                tailMin = loadInt(t + 48),
                tailMax = if (isSome(t + 52)) loadInt(t + 56) else null,
                separator = optionalString(t + 60),
            ).bounded(types),
        )
    }
    records(body + 88, OPTION_SIZE) { optionArgument(it, types) }.forEach { args.add(it) }
    records(body + 96, FLAG_SIZE, ::flagArgument).forEach { args.add(it) }
    val annotations = body + 288
    val types0 = HashMap<String, String>(globalTypes)
    args.forEach { a -> a.witType?.let { types0[a.name] = it } }
    val result = body + 232
    val formatters = if (isSome(result)) records(result + 4 + 28, 32) { str(it) } else emptyList()
    return ToolBodyInfo(
        args,
        isSome(body + 112),
        isSome(body + 152),
        isSome(body + 192),
        isSome(body + 232),
        records(body + 280, ERROR_SIZE) { str(it) },
        if (isSome(annotations)) {
            val a = annotations + 1
            ToolAnnotationsInfo(
                loadByte(a).toInt() != 0,
                loadByte(a + 1).toInt() != 0,
                loadByte(a + 2).toInt() != 0,
                loadByte(a + 3).toInt() != 0,
            )
        } else {
            null
        },
        records(body + 104, ArgLayout.CONSTRAINT_SIZE) { liftConstraint(it) { name -> types0[name] } },
        formatters,
        if (isSome(result)) str(result + 4 + 36) else null,
    )
}

private fun liftExamples(doc: Int): List<ToolExample> = records(doc + 16, ArgLayout.EXAMPLE_SIZE) { ToolExample(str(it), str(it + 8)) }

private fun liftNode(n: Int, types: TypeResolver, globalTypes: Map<String, String>): ToolCommandInfo {
    val globals = ArrayList<ToolArgumentInfo>()
    records(n + 40, OPTION_SIZE) { optionArgument(it, types) }.forEach { globals.add(it) }
    records(n + 48, FLAG_SIZE, ::flagArgument).forEach { globals.add(it) }
    val subs = loadInt(n + 56)
    return ToolCommandInfo(
        str(n),
        strings(n + 8),
        summary(n + 16),
        (0 until loadInt(n + 60)).map { loadInt(subs + it * 4) },
        globals,
        if (isSome(n + 64)) liftBody(n + 68, types, globalTypes) else null,
        liftExamples(n + 16),
    )
}

/** Decodes the `tool` record at [tool] (registered-tool + 8). */
internal fun liftToolInfo(registration: RegisteredTool, tool: Int): ToolInfo {
    // tool.schema (schema-graph): type-node list ptr@0, definitions list ptr@8
    val types = TypeResolver(loadInt(tool + 20), loadInt(tool + 28))
    val nodes = loadInt(tool + 12)
    val count = loadInt(tool + 16)
    val lifted = arrayOfNulls<ToolCommandInfo>(count)

    // A constraint value is typed by the globals of the command's own enclosing groups, so the map follows the path from the root.
    fun visit(index: Int, inherited: Map<String, String>) {
        val n = nodes + index * NODE_SIZE
        val own = HashMap(inherited)
        records(n + 40, OPTION_SIZE) { optionArgument(it, types) }.forEach { a -> a.witType?.let { own[a.name] = it } }
        lifted[index] = liftNode(n, types, own)
        val subs = loadInt(n + 56)
        for (j in 0 until loadInt(n + 60)) visit(loadInt(subs + j * 4), own)
    }
    if (count > 0) visit(0, emptyMap())
    for (i in 0 until count) if (lifted[i] == null) lifted[i] = liftNode(nodes + i * NODE_SIZE, types, emptyMap())
    return ToolInfo(registration, loadByte(tool + 8).toInt() != 0, lifted.map { checkNotNull(it) })
}
