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
class NativeToolParamSchema(val name: String, val witType: String)

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
    val handler: (List<SchemaValue>) -> SchemaValue,
)

/**
 * Canonical-ABI layout constants for `golem:tool@0.1.0`'s `common` interface, computed via
 * `wit-parser::SizeAlign` against `wit-native/deps/golem-tool/common.wit` on 2026-07-07 (see
 * `AgentTypeModel.kt`'s `Layout` doc comment for why these are tool-verified, not hand-derived).
 * `schema-graph`/`schema-type-node`/`schema-type-body`/`metadata-envelope` are shared with
 * `golem:agent/common@2.0.0` (both packages reference the same `golem:core/types@2.0.0`), so
 * this reuses `AgentTypeModel.kt`'s `lowerSchemaGraphInto`/`lowerSchemaTypeNodeInto`/
 * `lowerSchemaTypeBodyInto`/`lowerEmptyMetadataInto` rather than duplicating them.
 */
private object ToolLayout {
    // record doc: size=24 align=4
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

    // record command-body: size=256 align=4
    const val COMMAND_BODY_SIZE = 256
    const val COMMAND_BODY_ALIGN = 4
    const val CB_POSITIONALS = 0
    const val CB_OPTIONS = 88
    const val CB_FLAGS = 96
    const val CB_CONSTRAINTS = 104
    const val CB_STDIN = 112
    const val CB_STDOUT = 152
    const val CB_RESULT = 192
    const val CB_ERRORS = 240
    const val CB_ANNOTATIONS = 248

    // record command-node: size=324 align=4
    const val COMMAND_NODE_SIZE = 324
    const val COMMAND_NODE_ALIGN = 4
    const val CN_NAME = 0
    const val CN_ALIASES = 8
    const val CN_DOC = 16
    const val CN_GLOBALS = 40
    const val CN_SUBCOMMANDS = 56
    const val CN_BODY = 64

    // option<command-body> payload_offset = align_to(1, 4) = 4
    const val COMMAND_BODY_OPTION_PAYLOAD_OFFSET = 4

    // record command-tree: size=8 align=4 (inline record: nodes: list<command-node>)
    const val COMMAND_TREE_SIZE = 8
    const val CT_NODES = 0

    // record tool: size=36 align=4
    const val TOOL_SIZE = 36
    const val TOOL_ALIGN = 4
    const val T_VERSION = 0
    const val T_COMMANDS = 8
    const val T_SCHEMA = 16
}

/**
 * Lowers a [NativeToolDescriptor] to the canonical-ABI `tool` record (golem:tool@0.1.0#common),
 * returning a pointer to it. One command-node (the root), fixed positionals only.
 */
fun lowerTool(descriptor: NativeToolDescriptor): Int {
    val roots = buildList {
        descriptor.params.forEach { add(it.witType) }
        if (descriptor.outputWitType != "()") add(descriptor.outputWitType)
    }
    val typeIndex = collectTypeNodes(roots)

    val tool = alloc(ToolLayout.TOOL_SIZE, ToolLayout.TOOL_ALIGN)
    writeStringField(tool, ToolLayout.T_VERSION, "0.1.0")
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
    lowerCommandBodySomeInto(base, ToolLayout.CN_BODY, d, typeIndex)
}

private fun lowerDocInto(base: Int, offset: Int, summary: String) {
    val docBase = base + offset
    writeStringField(docBase, ToolLayout.DOC_SUMMARY, summary)
    writeStringField(docBase, ToolLayout.DOC_DESCRIPTION, "")
    writeEmptyListField(docBase, ToolLayout.DOC_EXAMPLES)
}

private fun lowerEmptyGlobalsInto(base: Int, offset: Int) {
    val globalsBase = base + offset
    writeEmptyListField(globalsBase, ToolLayout.GLOBALS_OPTIONS)
    writeEmptyListField(globalsBase, ToolLayout.GLOBALS_FLAGS)
}

private fun lowerCommandBodySomeInto(base: Int, offset: Int, d: NativeToolDescriptor, typeIndex: Map<String, Int>) {
    val optBase = base + offset
    storeByte(optBase, 1) // some
    val bodyBase = optBase + ToolLayout.COMMAND_BODY_OPTION_PAYLOAD_OFFSET

    lowerPositionalsInto(bodyBase, ToolLayout.CB_POSITIONALS, d.params, typeIndex)
    writeEmptyListField(bodyBase, ToolLayout.CB_OPTIONS)
    writeEmptyListField(bodyBase, ToolLayout.CB_FLAGS)
    writeEmptyListField(bodyBase, ToolLayout.CB_CONSTRAINTS)
    writeOptionNone(bodyBase, ToolLayout.CB_STDIN)
    writeOptionNone(bodyBase, ToolLayout.CB_STDOUT)
    if (d.outputWitType == "()") {
        writeOptionNone(bodyBase, ToolLayout.CB_RESULT)
    } else {
        lowerResultSpecSomeInto(bodyBase, ToolLayout.CB_RESULT, typeIndex.getValue(d.outputWitType))
    }
    writeEmptyListField(bodyBase, ToolLayout.CB_ERRORS)
    writeOptionNone(bodyBase, ToolLayout.CB_ANNOTATIONS)
}

private fun lowerPositionalsInto(base: Int, offset: Int, params: List<NativeToolParamSchema>, typeIndex: Map<String, Int>) {
    val posBase = base + offset
    writeListField(
        posBase,
        ToolLayout.POSITIONALS_FIXED,
        params.size,
        ToolLayout.POSITIONAL_SIZE,
        ToolLayout.POSITIONAL_ALIGN,
    ) { i, p -> lowerPositionalInto(p, params[i], typeIndex) }
    writeOptionNone(posBase, ToolLayout.POSITIONALS_TAIL)
}

private fun lowerPositionalInto(base: Int, p: NativeToolParamSchema, typeIndex: Map<String, Int>) {
    writeStringField(base, ToolLayout.POS_NAME, p.name)
    lowerDocInto(base, ToolLayout.POS_DOC, "")
    writeOptionNone(base, ToolLayout.POS_VALUE_NAME)
    storeInt(base + ToolLayout.POS_TYPE, typeIndex.getValue(p.witType))
    writeOptionNone(base, ToolLayout.POS_DEFAULT)
    storeByte(base + ToolLayout.POS_REQUIRED, 1) // true: no optional positionals
    storeByte(base + ToolLayout.POS_ACCEPTS_STDIO, 0) // false
}

private fun lowerResultSpecSomeInto(base: Int, offset: Int, outputTypeIndex: Int) {
    val optBase = base + offset
    storeByte(optBase, 1) // some
    val rsBase = optBase + ToolLayout.RESULT_SPEC_OPTION_PAYLOAD_OFFSET
    storeInt(rsBase + ToolLayout.RS_TYPE, outputTypeIndex)
    lowerDocInto(rsBase, ToolLayout.RS_DOC, "")
    // default-formatter must resolve to a name in formatters (construction invariant) -- a
    // single "default" formatter with no special rendering hint satisfies it.
    writeListField(
        rsBase,
        ToolLayout.RS_FORMATTERS,
        1,
        ToolLayout.FORMATTER_SIZE,
        ToolLayout.FORMATTER_ALIGN,
    ) { _, fmtPtr ->
        writeStringField(fmtPtr, ToolLayout.FMT_NAME, "default")
        lowerDocInto(fmtPtr, ToolLayout.FMT_DOC, "")
    }
    writeStringField(rsBase, ToolLayout.RS_DEFAULT_FORMATTER, "default")
}
