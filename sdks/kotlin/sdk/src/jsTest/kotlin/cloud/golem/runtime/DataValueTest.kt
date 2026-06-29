package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class DataValueTest {

    // Build the 2.0.0 `schema-value-tree` the host passes as a call's input: the root is a
    // `record-value` whose children are the parameters. Here, one parameter node at index 0.
    //   { valueNodes: [ {tag, val}, {tag:'record-value', val:[0]} ], root: 1 }
    private fun primInputTree(nodeTag: String, raw: dynamic): dynamic {
        val node = js("{}")
        node.tag = nodeTag
        node.`val` = raw
        val root = js("{}")
        root.tag = "record-value"
        root.`val` = arrayOf(0)
        val tree = js("{}")
        tree.valueNodes = arrayOf(node, root)
        tree.root = 1
        return tree
    }

    @Test
    fun extractStringRoundTrip() {
        val tree = primInputTree("string-value", "hello-counter")
        assertEquals("hello-counter", extractString(tree, 0))
    }

    @Test
    fun extractIntRoundTrip() {
        val tree = primInputTree("s32-value", 42)
        assertEquals(42, extractInt(tree, 0))
    }

    @Test
    fun wrapIntProducesSingleS32Tree() {
        val wrapped = wrapInt(7)
        assertEquals(0, wrapped.root as Int)
        val node = wrapped.valueNodes[0]
        assertEquals("s32-value", node.tag as String)
        assertEquals(7, node.`val` as Int)
    }

    @Test
    fun wrapStringProducesSingleStringTree() {
        val wrapped = wrapString("test-name")
        assertEquals(0, wrapped.root as Int)
        val node = wrapped.valueNodes[0]
        assertEquals("string-value", node.tag as String)
        assertEquals("test-name", node.`val` as String)
    }

    @Test
    fun wrapUnitProducesNone() {
        // unit output is `none`, lowered to JS undefined (compares == null in Kotlin/JS).
        assertNull(wrapUnit())
    }

    @Test
    fun extractLongRoundTrip() {
        val tree = primInputTree("s64-value", js("BigInt")("123456789"))
        assertEquals(123456789L, extractLong(tree, 0))
    }
}
