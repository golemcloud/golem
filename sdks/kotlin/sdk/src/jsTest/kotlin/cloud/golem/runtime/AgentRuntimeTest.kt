package cloud.golem.runtime

import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

// A mock agent for testing
class MockAgent_Task4(val name: String) {
    var callCount = 0
    fun ping(): Int {
        callCount++
        return callCount
    }
}

class AgentRuntimeTest {

    // Build a 2.0.0 constructor/method input tree: a `record-value` root over the parameters.
    private fun ctorInput(name: String): dynamic {
        val node = js("{}"); node.tag = "string-value"; node.`val` = name
        val root = js("{}"); root.tag = "record-value"; root.`val` = arrayOf(0)
        val tree = js("{}"); tree.valueNodes = arrayOf(node, root); tree.root = 1
        return tree
    }

    // Empty parameter list: a `record-value` root with no children.
    private fun emptyInput(): dynamic {
        val root = js("{}"); root.tag = "record-value"; root.`val` = emptyArray<dynamic>()
        val tree = js("{}"); tree.valueNodes = arrayOf(root); tree.root = 0
        return tree
    }

    @BeforeTest
    fun setup() {
        // GolemAgentRuntime is a singleton; clear the agent constructed by a
        // prior test so initialize() (which now rejects a second construction)
        // starts fresh each time.
        GolemAgentRuntime.currentAgent = null
        // Register a fresh mock agent type for each test
        GolemAgentRuntime.registerAgent(
            AgentDescriptor(
                typeName = "MockAgent_Task4",
                description = "A mock agent for testing",
                mountPath = "/mock/{name}",
                constructorParams = listOf(ParamSchema("name", "string")),
                methods = listOf(
                    MethodDescriptor("ping", "s32", emptyList(), emptyList()) { instance, _ ->
                        val agent = instance as MockAgent_Task4
                        wrapInt(agent.ping())
                    }
                ),
                factory = { input ->
                    val agentName = extractString(input, 0)
                    MockAgent_Task4(agentName)
                }
            )
        )
    }

    // The host wrapper (call_js_export_returning_result) maps a normal return to the
    // WIT `ok` case and a thrown value to `err`. So the guest functions return BARE
    // values (undefined for unit, the schema-value-tree for invoke, the array for
    // discover) and THROW the agent-error object on failure — never {tag,val}.

    @Test
    fun initializeCreatesAgent() {
        // No throw on success; currentAgent is set.
        GolemAgentRuntime.initialize("MockAgent_Task4", ctorInput("test-instance"), null)
        assertNotNull(GolemAgentRuntime.currentAgent)
    }

    @Test
    fun invokeDispatchesToMethod() {
        GolemAgentRuntime.initialize("MockAgent_Task4", ctorInput("test-instance-2"), null)
        // invoke returns the bare single-value tree wrapInt(1):
        //   { valueNodes: [ {tag:'s32-value', val:1} ], root: 0 }
        val result = GolemAgentRuntime.invoke("ping", emptyInput(), null)
        assertEquals(0, result.root as Int)
        assertEquals("s32-value", result.valueNodes[0].tag as String)
        assertEquals(1, result.valueNodes[0].`val` as Int)
    }

    @Test
    fun invokeUnknownMethodThrows() {
        GolemAgentRuntime.initialize("MockAgent_Task4", ctorInput("test-instance-3"), null)
        // Unknown method throws the agent-error JS object { tag:"invalid-method", val:"..." }.
        var caughtTag: String? = null
        try {
            GolemAgentRuntime.invoke("nonexistent", emptyInput(), null)
        } catch (e: dynamic) {
            caughtTag = e.tag as? String
        }
        assertEquals("invalid-method", caughtTag)
    }

    @Test
    fun discoverAgentTypesReturnsRegistered() {
        // Bare array (not wrapped in {tag:"ok"}).
        val arr = GolemAgentRuntime.discoverAgentTypes() as Array<*>
        assertTrue(arr.isNotEmpty(), "Expected at least one agent type")
    }
}
