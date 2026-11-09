@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.wasm.alloc
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.writeEmptyListField
import cloud.golem.wasm.writeListField
import cloud.golem.wasm.writeOptionNone
import cloud.golem.wasm.writeStringField

/** One runnable command of a [NativeToolDefinition]: [path] is the names below the root; [outputWitType] `"()"` means no result. */
class NativeCommand(
    val path: List<String>,
    val description: String,
    val params: List<NativeToolParamSchema>,
    val outputWitType: String,
    val errors: List<NativeToolErrorCase>,
    val aliases: List<String> = emptyList(),
    val examples: List<ToolExample> = emptyList(),
    /** Tail, options, flags, constraints, formatters and hints; [params] are the fixed positionals. */
    val extras: NativeBodyExtras = NativeBodyExtras.NONE,
)

/** A command group (a bodiless node) at [path]: its [options] and [flags] are globals, inherited by every command below it. */
class NativeGroup(
    val path: List<String>,
    val description: String = "",
    val aliases: List<String> = emptyList(),
    val options: List<NativeArg> = emptyList(),
    val flags: List<NativeArg> = emptyList(),
)

/** A tool described by a `@ToolDefinition` interface: the root names the tool and groups the [commands]. */
class NativeToolDefinition(
    val name: String,
    val version: String,
    val summary: String,
    val requiresFilesystem: Boolean,
    val commands: List<NativeCommand>,
    val groups: List<NativeGroup> = emptyList(),
)

private class CommandTreeNode(val name: String, val command: NativeCommand?, val group: NativeGroup? = null) {
    val children = ArrayList<CommandTreeNode>()
}

private fun buildCommandTree(d: NativeToolDefinition): List<CommandTreeNode> {
    val root = CommandTreeNode(d.name, null, d.groups.firstOrNull { it.path.isEmpty() })
    for (c in d.commands) {
        require(c.path.isNotEmpty()) { "tool '${d.name}' has a command with an empty path" }
        var node = root
        for ((i, step) in c.path.withIndex()) {
            val last = i == c.path.lastIndex
            val existing = node.children.firstOrNull { it.name == step }
            node = when {
                existing == null -> CommandTreeNode(step, if (last) c else null, if (last) null else d.groups.firstOrNull { it.path == c.path.take(i + 1) })
                    .also { node.children.add(it) }
                last -> error("tool '${d.name}' declares command ${c.path} twice")
                else -> existing
            }
        }
    }
    val flat = ArrayList<CommandTreeNode>()
    fun visit(n: CommandTreeNode) {
        flat.add(n)
        n.children.forEach(::visit)
    }
    visit(root)
    return flat
}

/**
 * Lowers a [NativeToolDefinition] to the canonical-ABI `tool` record. Node 0 is the root (no body); every command path is a node reached
 * from it, with nested groups as bodiless intermediate nodes.
 */
fun lowerToolDefinition(d: NativeToolDefinition): Int {
    val tool = alloc(ToolLayout.TOOL_SIZE, ToolLayout.TOOL_ALIGN)
    lowerToolDefinitionInto(tool, d)
    return tool
}

/** Writes the `tool` record (40 B) for [d] at [tool]. */
internal fun lowerToolDefinitionInto(tool: Int, d: NativeToolDefinition) {
    val roots = buildList {
        for (c in d.commands) {
            c.params.forEach { add(it.witType) }
            addAll(c.extras.typeRoots())
            if (c.outputWitType != "()") add(c.outputWitType)
            c.errors.forEach { e -> e.payloadWitType?.let { add(it) } }
        }
        for (g in d.groups) addAll((g.options + g.flags).mapNotNull { it.nodeWitType })
    }
    val typeIndex = collectTypeNodes(roots)
    val nodes = buildCommandTree(d)
    val indexOf = HashMap<CommandTreeNode, Int>().also { m -> nodes.forEachIndexed { i, n -> m[n] = i } }

    writeStringField(tool, ToolLayout.T_VERSION, d.version)
    storeByte(tool + ToolLayout.T_REQUIRES_FILESYSTEM, (if (d.requiresFilesystem) 1 else 0).toByte())
    writeListField(tool + ToolLayout.T_COMMANDS, ToolLayout.CT_NODES, nodes.size, ToolLayout.COMMAND_NODE_SIZE, ToolLayout.COMMAND_NODE_ALIGN) { i, ptr ->
        val node = nodes[i]
        writeStringField(ptr, ToolLayout.CN_NAME, node.name)
        val aliases = node.command?.aliases ?: node.group?.aliases ?: emptyList()
        if (aliases.isEmpty()) {
            writeEmptyListField(ptr, ToolLayout.CN_ALIASES)
        } else {
            writeListField(ptr, ToolLayout.CN_ALIASES, aliases.size, 8, 4) { j, p -> writeStringField(p, 0, aliases[j]) }
        }
        lowerDocInto(ptr, ToolLayout.CN_DOC, if (i == 0) d.summary else node.command?.description ?: node.group?.description ?: "", node.command?.examples ?: emptyList())
        lowerGlobalsInto(ptr, ToolLayout.CN_GLOBALS, node.group?.options ?: emptyList(), node.group?.flags ?: emptyList(), typeIndex)
        writeListField(ptr, ToolLayout.CN_SUBCOMMANDS, node.children.size, 4, 4) { j, p -> storeInt(p, indexOf.getValue(node.children[j])) }
        val c = node.command
        if (c == null) {
            writeOptionNone(ptr, ToolLayout.CN_BODY)
        } else {
            lowerCommandBodySomeInto(ptr, ToolLayout.CN_BODY, c.params, c.outputWitType, c.extras.stdin, c.extras.stdout, c.extras.stderr, c.errors, typeIndex, c.extras)
        }
    }
    lowerSchemaGraphInto(tool, ToolLayout.T_SCHEMA, typeIndex)
}
