package cloud.golem.runtime

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull

class NamedRegistryTest {
    private fun registry(sorted: Boolean = false) = NamedRegistry<String>("entry", { it }, sorted)

    @Test
    fun a_duplicate_registration_is_rejected() {
        val registry = registry()
        registry.register("dupe")
        assertFailsWith<IllegalStateException> { registry.register("dupe") }
        assertEquals(listOf("dupe"), registry.all())
    }

    @Test
    fun discovery_is_in_registration_order_or_sorted_and_lookup_is_by_name() {
        val unsorted = registry()
        val sorted = registry(sorted = true)
        for (name in listOf("registry-sort-second", "registry-sort-first")) {
            unsorted.register(name)
            sorted.register(name)
        }
        assertEquals(listOf("registry-sort-second", "registry-sort-first"), unsorted.all())
        assertEquals(listOf("registry-sort-first", "registry-sort-second"), sorted.all())
        assertEquals("registry-sort-first", sorted.lookup("registry-sort-first"))
        assertNull(sorted.lookup("missing"))
    }

    @Test
    fun registries_are_independent_namespaces() {
        val tools = registry()
        val middleware = registry(sorted = true)
        middleware.register("registry-shared-name")
        assertNull(tools.lookup("registry-shared-name"))
        tools.register("registry-shared-name")
        assertEquals("registry-shared-name", middleware.lookup("registry-shared-name"))
    }
}
