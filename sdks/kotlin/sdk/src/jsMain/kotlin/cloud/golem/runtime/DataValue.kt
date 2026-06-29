@file:OptIn(ExperimentalJsExport::class)
package cloud.golem.runtime

import kotlin.js.JsExport
import kotlin.js.ExperimentalJsExport

/**
 * Helpers for converting between Kotlin values and the `schema-value-tree` wire format
 * (golem:core/types@2.0.0) that the QuickJS host passes across the `golem:agent/guest@2.0.0`
 * boundary.
 *
 * Wire format (grounded in golem-core-v2.wit + the Scala SDK facades, NOT guessed):
 *   schema-value-tree -> { valueNodes: [ <schema-value-node>... ], root: <index> }
 *   schema-value-node is a {tag, val} variant; primitive cases:
 *     s32-value(s32)        -> { tag: "s32-value",    val: <number> }
 *     s64-value(s64)        -> { tag: "s64-value",    val: <bigint> }
 *     string-value(string)  -> { tag: "string-value", val: <string> }
 *     record-value(list<i>) -> { tag: "record-value", val: [ <index>... ] }
 *   Composite payloads are arrays of integer indices into `valueNodes`; nodes are flattened
 *   children-first (a parent's index is higher than its children's).
 *
 * INPUT contract: the `input` tree's root is ALWAYS a `record-value` whose ordered children are
 * the call's parameters (one field per declared parameter, in declaration order). So parameter
 * `i` is `valueNodes[ valueNodes[root].val[i] ]`.
 *
 * OUTPUT contract: a value-returning method returns a `single` value tree whose root IS the
 * value node (NOT wrapped in a tuple/record). A unit return is `none` -> `undefined`.
 */

/** Read the value node for parameter [index] out of the input tree's record-value root. */
private fun paramNode(tree: dynamic, index: Int): dynamic {
    val rootNode = tree.valueNodes[tree.root]
    val childIndex = rootNode.`val`[index]
    return tree.valueNodes[childIndex]
}

/** Extract a String (string-value) parameter at [index]. */
@JsExport
fun extractString(tree: dynamic, index: Int = 0): String {
    return paramNode(tree, index).`val` as String
}

/** Extract an Int (s32-value) parameter at [index]. */
@JsExport
fun extractInt(tree: dynamic, index: Int = 0): Int {
    return (paramNode(tree, index).`val`).unsafeCast<Int>()
}

/** Extract a Long (s64-value) parameter at [index]. s64 crosses the boundary as a JS bigint. */
fun extractLong(tree: dynamic, index: Int = 0): Long {
    return (paramNode(tree, index).`val`).toString().toLong()
}

/** Build a single-value `schema-value-tree` whose root is the given node. */
private fun singleValueTree(node: dynamic): dynamic {
    val tree = js("{}")
    tree.valueNodes = arrayOf(node)
    tree.root = 0
    return tree
}

/** Wrap a String into a `single` string-value output tree. */
@JsExport
fun wrapString(s: String): dynamic {
    val node = js("{}")
    node.tag = "string-value"
    node.`val` = s
    return singleValueTree(node)
}

/** Wrap an Int into a `single` s32-value output tree. */
@JsExport
fun wrapInt(n: Int): dynamic {
    val node = js("{}")
    node.tag = "s32-value"
    node.`val` = n
    return singleValueTree(node)
}

/** Wrap a Long into a `single` s64-value output tree (s64 is a JS bigint). */
fun wrapLong(n: Long): dynamic {
    val node = js("{}")
    node.tag = "s64-value"
    node.`val` = js("BigInt")(n.toString())
    return singleValueTree(node)
}

/**
 * A unit return value. `invoke` returns `option<schema-value-tree>`, which is `none` for a unit
 * output; `none` is lowered to `undefined` at the function boundary.
 */
@JsExport
fun wrapUnit(): dynamic {
    return js("undefined")
}
