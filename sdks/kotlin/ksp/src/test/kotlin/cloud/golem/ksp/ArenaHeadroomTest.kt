package cloud.golem.ksp

import com.tschuchort.compiletesting.SourceFile
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertFalse
import kotlin.test.assertTrue

// A component sets the arena headroom with the KSP option `golem.arenaHeadroomKiB`. Without it the generated code is unchanged; with it the
// generated entry point sets the headroom before anything allocates: at module start, in cabi_realloc and in every registration function.
class ArenaHeadroomTest {
    private val agent = SourceFile.kotlin(
        "CounterAgent.kt",
        """
        package counter
        import cloud.golem.annotations.Agent
        import cloud.golem.annotations.Endpoint
        import cloud.golem.BaseAgent

        @Agent(mount = "/counters/{name}", description = "A counter")
        class CounterAgent(val name: String) : BaseAgent() {
            @Endpoint(get = "/value")
            fun value(): Int = 1
        }
        """.trimIndent(),
    )

    @Test
    fun without_the_option_the_entry_point_does_not_mention_the_headroom() {
        val entry = generated(runProcessor(agent), "GolemGeneratedGuest.kt")
        assertFalse("configureArenaHeadroom" in entry, entry)
    }

    @Test
    fun the_option_sets_the_headroom_at_module_start_in_cabi_realloc_and_in_every_registration() {
        val entry = generated(runProcessor(agent, options = mapOf("golem.arenaHeadroomKiB" to "256")), "GolemGeneratedGuest.kt")
        val call = "cloud.golem.wasm.configureArenaHeadroom(262144)"
        assertContains(entry, "private val golemArenaHeadroom = $call")
        val realloc = entry.substringAfter("fun golemGuestCabiRealloc").substringBefore("private var golemAgentsRegistered")
        assertContains(realloc, call)
        for (register in listOf("registerAllAgents", "registerAllTools", "registerAllToolMiddlewares")) {
            val body = entry.substringAfter("fun $register() {").substringBefore("\n}")
            assertTrue(body.trimStart().startsWith(call), "$register should set the headroom first: $body")
        }
    }

    @Test
    fun the_smallest_and_largest_headroom_are_accepted() {
        for ((kib, bytes) in listOf("64" to 65536, "16384" to 16777216)) {
            val entry = generated(runProcessor(agent, options = mapOf("golem.arenaHeadroomKiB" to kib)), "GolemGeneratedGuest.kt")
            assertContains(entry, "configureArenaHeadroom($bytes)")
        }
    }

    @Test
    fun a_headroom_that_is_not_whole_pages_or_is_out_of_range_is_an_error_naming_the_option() {
        for (bad in listOf("0", "32", "100", "16448", "-64", "abc", "")) {
            val messages = failureMessages(agent, options = mapOf("golem.arenaHeadroomKiB" to bad))
            assertContains(messages, "golem.arenaHeadroomKiB", message = "for '$bad': $messages")
        }
    }
}
