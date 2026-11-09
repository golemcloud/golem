@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.writeEmptyListField
import cloud.golem.wasm.writeListField
import cloud.golem.wasm.writeOptionNone
import cloud.golem.wasm.writeStringField

/**
 * A single tool parameter: mirrors [NativeParamSchema], but named separately since a tool's
 * parameters lower to `positional`s, not `named-field`s in an input-schema `record`.
 */
class NativeToolParamSchema(
    val name: String,
    val witType: String,
    val doc: String = "",
    val valueName: String? = null,
    val acceptsStdio: Boolean = false,
)

/**
 * A native `golem:tool@0.1.0` tool, scoped to one root command (no subcommands, no
 * options/flags/constraints/streams/errors). Parameters lower 1:1 to fixed positionals, in
 * declaration order; `outputWitType == "()"` means no `result-spec`.
 */
class NativeToolDescriptor(
    val name: String,
    val description: String,
    val params: List<NativeToolParamSchema>,
    val outputWitType: String,
    /** Declares the optional `stdin` / `stdout` / `stderr` stream specs; the body reaches them through [ToolStreams]. */
    val stdin: Boolean = false,
    val stdout: Boolean = false,
    val stderr: Boolean = false,
    /** The declared error cases a body may raise with [ToolCustomError]. */
    val errors: List<NativeToolErrorCase> = emptyList(),
    val handler: (List<SchemaValue>, ToolStreams) -> SchemaValue,
)

/** One declared `error-case` of a tool; [payloadWitType] is `null` for a payloadless error. */
class NativeToolErrorCase(
    val name: String,
    val description: String = "",
    val payloadWitType: String? = null,
    val usage: Boolean = false,
    val exitCode: Int = 1,
)

/**
 * Canonical-ABI layout constants for `golem:tool@0.1.0`'s `common` interface, computed from
 * `wasm-tools component wit --json` of the 1.6.0 `wit-native/deps/golem-tool/common.wit` on 2026-10-21 (earlier revision: wit-parser::SizeAlign on 2026-07-07; see
 * `AgentTypeModel.kt`'s `Layout` doc comment for why these are tool-verified, not hand-derived).
 * `schema-graph`/`schema-type-node`/`schema-type-body`/`metadata-envelope` are shared with
 * `golem:agent/common@2.0.0` (both packages reference the same `golem:core/types@2.0.0`), so
 * this reuses `AgentTypeModel.kt`'s `lowerSchemaGraphInto`/`lowerSchemaTypeNodeInto`/
 * `lowerSchemaTypeBodyInto`/`lowerEmptyMetadataInto` rather than duplicating them.
 */
internal object ToolLayout {
    // record doc: size=24 align=4
    // record error-case: size=44 align=4
    const val ERROR_CASE_SIZE = 44
    const val ERROR_CASE_ALIGN = 4

    const val DOC_SIZE = 24
    const val DOC_ALIGN = 4
    const val DOC_SUMMARY = 0
    const val DOC_DESCRIPTION = 8
    const val DOC_EXAMPLES = 16

    // record positional: size=68 align=4
    const val POSITIONAL_SIZE = 68
    const val POSITIONAL_ALIGN = 4
    const val POS_NAME = 0
    const val POS_DOC = 8
    const val POS_VALUE_NAME = 32
    const val POS_TYPE = 44
    const val POS_DEFAULT = 48
    const val POS_REQUIRED = 64
    const val POS_ACCEPTS_STDIO = 65

    // record positionals: size=88 align=4
    const val POSITIONALS_FIXED = 0
    const val POSITIONALS_TAIL = 8

    // record globals: size=16 align=4
    const val GLOBALS_OPTIONS = 0
    const val GLOBALS_FLAGS = 8

    // record result-spec: size=44 align=4
    const val RESULT_SPEC_SIZE = 44
    const val RS_TYPE = 0
    const val RS_DOC = 4
    const val RS_FORMATTERS = 28
    const val RS_DEFAULT_FORMATTER = 36

    // record formatter: size=32 align=4
    const val FORMATTER_SIZE = 32
    const val FORMATTER_ALIGN = 4
    const val FMT_NAME = 0
    const val FMT_DOC = 8

    // option<result-spec> payload_offset = align_to(1, 4) = 4
    const val RESULT_SPEC_OPTION_PAYLOAD_OFFSET = 4

    // record command-body: size=296 align=4
    const val COMMAND_BODY_SIZE = 296
    const val COMMAND_BODY_ALIGN = 4
    const val CB_POSITIONALS = 0
    const val CB_OPTIONS = 88
    const val CB_FLAGS = 96
    const val CB_CONSTRAINTS = 104
    const val CB_STDIN = 112
    const val CB_STDOUT = 152
    const val CB_STDERR = 192
    const val CB_RESULT = 232
    const val CB_ERRORS = 280
    const val CB_ANNOTATIONS = 288

    // record command-node: size=364 align=4
    const val COMMAND_NODE_SIZE = 364
    const val COMMAND_NODE_ALIGN = 4
    const val CN_NAME = 0
    const val CN_ALIASES = 8
    const val CN_DOC = 16
    const val CN_GLOBALS = 40
    const val CN_SUBCOMMANDS = 56
    const val CN_BODY = 64

    // record stream-spec: size=36 align=4 { doc@0 (24), mime: list<string>@24, required: bool@32 }
    const val STREAM_SPEC_OPTION_PAYLOAD_OFFSET = 4
    const val SS_DOC = 0
    const val SS_MIME = 24
    const val SS_REQUIRED = 32

    // option<command-body> payload_offset = align_to(1, 4) = 4
    const val COMMAND_BODY_OPTION_PAYLOAD_OFFSET = 4

    // record command-tree: size=8 align=4 (inline record: nodes: list<command-node>)
    const val COMMAND_TREE_SIZE = 8
    const val CT_NODES = 0

    // record tool: size=40 align=4
    const val TOOL_SIZE = 40
    const val TOOL_ALIGN = 4
    const val T_VERSION = 0
    const val T_REQUIRES_FILESYSTEM = 8
    const val T_COMMANDS = 12
    const val T_SCHEMA = 20
}

/**
 * Lowers a [NativeToolDescriptor] to the canonical-ABI `tool` record (golem:tool@0.1.0#common),
 * returning a pointer to it. One command-node (the root), fixed positionals only.
 */
fun lowerTool(descriptor: NativeToolDescriptor): Int {
    val roots = buildList {
        descriptor.params.forEach { add(it.witType) }
        if (descriptor.outputWitType != "()") add(descriptor.outputWitType)
        descriptor.errors.forEach { e -> e.payloadWitType?.let { add(it) } }
    }
    val typeIndex = collectTypeNodes(roots)

    val tool = alloc(ToolLayout.TOOL_SIZE, ToolLayout.TOOL_ALIGN)
    writeStringField(tool, ToolLayout.T_VERSION, "0.1.0")
    storeByte(tool + ToolLayout.T_REQUIRES_FILESYSTEM, 0) // false: no filesystem access
    lowerCommandTreeInto(tool, ToolLayout.T_COMMANDS, descriptor, typeIndex)
    lowerSchemaGraphInto(tool, ToolLayout.T_SCHEMA, typeIndex)
    return tool
}

private fun lowerCommandTreeInto(base: Int, offset: Int, d: NativeToolDescriptor, typeIndex: Map<String, Int>) {
    val treeBase = base + offset
    writeListField(
        treeBase,
        ToolLayout.CT_NODES,
        1,
        ToolLayout.COMMAND_NODE_SIZE,
        ToolLayout.COMMAND_NODE_ALIGN,
    ) { _, nodePtr -> lowerRootCommandNodeInto(nodePtr, d, typeIndex) }
}

private fun lowerRootCommandNodeInto(base: Int, d: NativeToolDescriptor, typeIndex: Map<String, Int>) {
    writeStringField(base, ToolLayout.CN_NAME, d.name)
    writeEmptyListField(base, ToolLayout.CN_ALIASES)
    lowerDocInto(base, ToolLayout.CN_DOC, d.description)
    lowerEmptyGlobalsInto(base, ToolLayout.CN_GLOBALS)
    writeEmptyListField(base, ToolLayout.CN_SUBCOMMANDS)
    lowerCommandBodySomeInto(base, ToolLayout.CN_BODY, d.params, d.outputWitType, d.stdin, d.stdout, d.stderr, d.errors, typeIndex)
}

internal fun lowerDocInto(base: Int, offset: Int, summary: String, examples: List<ToolExample> = emptyList()) {
    val docBase = base + offset
    writeStringField(docBase, ToolLayout.DOC_SUMMARY, summary)
    writeStringField(docBase, ToolLayout.DOC_DESCRIPTION, "")
    if (examples.isEmpty()) {
        writeEmptyListField(docBase, ToolLayout.DOC_EXAMPLES)
    } else {
        writeListField(docBase, ToolLayout.DOC_EXAMPLES, examples.size, ArgLayout.EXAMPLE_SIZE, 4) { i, ptr ->
            writeStringField(ptr, 0, examples[i].title)
            writeStringField(ptr, 8, examples[i].body)
        }
    }
}

internal fun lowerEmptyGlobalsInto(base: Int, offset: Int) {
    val globalsBase = base + offset
    writeEmptyListField(globalsBase, ToolLayout.GLOBALS_OPTIONS)
    writeEmptyListField(globalsBase, ToolLayout.GLOBALS_FLAGS)
}

internal fun lowerGlobalsInto(base: Int, offset: Int, options: List<NativeArg>, flags: List<NativeArg>, typeIndex: Map<String, Int>) {
    val globalsBase = base + offset
    lowerOptionsInto(globalsBase, ToolLayout.GLOBALS_OPTIONS, options, typeIndex)
    lowerFlagsInto(globalsBase, ToolLayout.GLOBALS_FLAGS, flags)
}

internal fun lowerCommandBodySomeInto(
    base: Int,
    offset: Int,
    params: List<NativeToolParamSchema>,
    outputWitType: String,
    stdin: Boolean,
    stdout: Boolean,
    stderr: Boolean,
    errors: List<NativeToolErrorCase>,
    typeIndex: Map<String, Int>,
    extras: NativeBodyExtras = NativeBodyExtras.NONE,
) {
    val optBase = base + offset
    storeByte(optBase, 1) // some
    val bodyBase = optBase + ToolLayout.COMMAND_BODY_OPTION_PAYLOAD_OFFSET

    lowerPositionalsInto(bodyBase, ToolLayout.CB_POSITIONALS, params, extras.tail, typeIndex)
    lowerOptionsInto(bodyBase, ToolLayout.CB_OPTIONS, extras.options, typeIndex)
    lowerFlagsInto(bodyBase, ToolLayout.CB_FLAGS, extras.flags)
    lowerConstraintsInto(bodyBase, ToolLayout.CB_CONSTRAINTS, extras.constraints)
    lowerStreamSpecInto(bodyBase, ToolLayout.CB_STDIN, stdin)
    lowerStreamSpecInto(bodyBase, ToolLayout.CB_STDOUT, stdout)
    lowerStreamSpecInto(bodyBase, ToolLayout.CB_STDERR, stderr)
    if (outputWitType == "()") {
        writeOptionNone(bodyBase, ToolLayout.CB_RESULT)
    } else {
        lowerResultSpecSomeInto(bodyBase, ToolLayout.CB_RESULT, typeIndex.getValue(outputWitType), extras.formatters, extras.defaultFormatter)
    }
    writeListField(bodyBase, ToolLayout.CB_ERRORS, errors.size, ToolLayout.ERROR_CASE_SIZE, ToolLayout.ERROR_CASE_ALIGN) { i, ptr ->
        lowerErrorCaseInto(ptr, errors[i], typeIndex)
    }
    lowerAnnotationsInto(bodyBase, ToolLayout.CB_ANNOTATIONS, extras.annotations)
}

/** `error-case`: name@0, doc@8, kind@32 (0 usage-error, 1 runtime-error), exit-code@33, payload option<type-node-index>@36. */
private fun lowerErrorCaseInto(base: Int, e: NativeToolErrorCase, typeIndex: Map<String, Int>) {
    writeStringField(base, 0, e.name)
    lowerDocInto(base, 8, e.description)
    storeByte(base + 32, (if (e.usage) 0 else 1).toByte())
    storeByte(base + 33, e.exitCode.toByte())
    val payload = e.payloadWitType
    if (payload == null) {
        writeOptionNone(base, 36)
    } else {
        storeByte(base + 36, 1)
        storeInt(base + 40, typeIndex.getValue(payload))
    }
}

/** `option<stream-spec>`: absent, or an optional (`required = false`) binary stream. */
private fun lowerStreamSpecInto(base: Int, offset: Int, declared: Boolean) {
    if (!declared) {
        writeOptionNone(base, offset)
        return
    }
    val optBase = base + offset
    storeByte(optBase, 1) // some
    val spec = optBase + ToolLayout.STREAM_SPEC_OPTION_PAYLOAD_OFFSET
    lowerDocInto(spec, ToolLayout.SS_DOC, "")
    writeListField(spec, ToolLayout.SS_MIME, 1, 8, 4) { _, ptr -> writeStringField(ptr, 0, "application/octet-stream") }
    storeByte(spec + ToolLayout.SS_REQUIRED, 0) // the caller may omit it
}

private fun lowerPositionalsInto(base: Int, offset: Int, params: List<NativeToolParamSchema>, tail: NativeArg?, typeIndex: Map<String, Int>) {
    val posBase = base + offset
    writeListField(
        posBase,
        ToolLayout.POSITIONALS_FIXED,
        params.size,
        ToolLayout.POSITIONAL_SIZE,
        ToolLayout.POSITIONAL_ALIGN,
    ) { i, p -> lowerPositionalInto(p, params[i], typeIndex) }
    lowerTailInto(posBase, ToolLayout.POSITIONALS_TAIL, tail, typeIndex)
}

private fun lowerPositionalInto(base: Int, p: NativeToolParamSchema, typeIndex: Map<String, Int>) {
    writeStringField(base, ToolLayout.POS_NAME, p.name)
    lowerDocInto(base, ToolLayout.POS_DOC, p.doc)
    lowerOptionString(base, ToolLayout.POS_VALUE_NAME, p.valueName)
    storeInt(base + ToolLayout.POS_TYPE, typeIndex.getValue(p.witType))
    writeOptionNone(base, ToolLayout.POS_DEFAULT)
    storeByte(base + ToolLayout.POS_REQUIRED, 1) // true: no optional positionals
    storeByte(base + ToolLayout.POS_ACCEPTS_STDIO, (if (p.acceptsStdio) 1 else 0).toByte())
}

private fun lowerResultSpecSomeInto(base: Int, offset: Int, outputTypeIndex: Int, formatters: List<String>, defaultFormatter: String) {
    val optBase = base + offset
    storeByte(optBase, 1) // some
    val rsBase = optBase + ToolLayout.RESULT_SPEC_OPTION_PAYLOAD_OFFSET
    storeInt(rsBase + ToolLayout.RS_TYPE, outputTypeIndex)
    lowerDocInto(rsBase, ToolLayout.RS_DOC, "")
    // default-formatter must resolve to a name in formatters (construction invariant).
    val names = formatters.ifEmpty { listOf("default") }
    require(defaultFormatter in names) { "default formatter '$defaultFormatter' is not one of $names" }
    writeListField(
        rsBase,
        ToolLayout.RS_FORMATTERS,
        names.size,
        ToolLayout.FORMATTER_SIZE,
        ToolLayout.FORMATTER_ALIGN,
    ) { i, fmtPtr ->
        writeStringField(fmtPtr, ToolLayout.FMT_NAME, names[i])
        lowerDocInto(fmtPtr, ToolLayout.FMT_DOC, "")
    }
    writeStringField(rsBase, ToolLayout.RS_DEFAULT_FORMATTER, defaultFormatter)
}
