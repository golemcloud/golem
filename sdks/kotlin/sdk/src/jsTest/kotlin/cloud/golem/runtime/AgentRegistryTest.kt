package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class AgentRegistryTest {

    // Reset state between tests by using unique names
    @Test
    fun registerAndLookupByName() {
        val descriptor = AgentDescriptor(
            typeName = "TestAgent_Task2",
            description = "A test agent",
            mountPath = "/test/{id}",
            constructorParams = emptyList(),
            methods = emptyList(),
            factory = { _ -> object {} }
        )
        AgentRegistry.register("TestAgent_Task2", descriptor)
        val found = AgentRegistry.lookup("TestAgent_Task2")
        assertNotNull(found)
        assertEquals("TestAgent_Task2", found.typeName)
    }

    @Test
    fun lookupMissingReturnsNull() {
        val found = AgentRegistry.lookup("DoesNotExist_Task2")
        assertNull(found)
    }

    @Test
    fun allReturnsRegistered() {
        val descriptor = AgentDescriptor(
            typeName = "AnotherAgent_Task2",
            description = "Another",
            mountPath = "/another/{id}",
            constructorParams = emptyList(),
            methods = emptyList(),
            factory = { _ -> object {} }
        )
        AgentRegistry.register("AnotherAgent_Task2", descriptor)
        val all = AgentRegistry.all()
        val names = all.map { it.typeName }
        assertTrue(names.contains("AnotherAgent_Task2"), "Expected AnotherAgent_Task2 in $names")
    }
}
