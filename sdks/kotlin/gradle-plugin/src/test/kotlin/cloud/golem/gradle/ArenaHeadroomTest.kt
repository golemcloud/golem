package cloud.golem.gradle

import org.gradle.api.internal.project.ProjectInternal
import org.gradle.api.provider.Provider
import org.gradle.testfixtures.ProjectBuilder
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Stands in for the `ksp { }` extension, which takes processor options through `arg(key, provider)`. */
open class FakeKspExtension {
    val args = mutableMapOf<String, Provider<String>>()

    fun arg(key: String, value: Provider<String>) {
        args[key] = value
    }
}

/**
 * `wasmComponent { arenaHeadroomKiB.set(256) }` reaches the KSP processor as the option `golem.arenaHeadroomKiB`, which makes the generated
 * entry point set the arena headroom. Using [ProjectBuilder], with no Kotlin/Wasm toolchain and no network.
 */
class ArenaHeadroomTest {
    private fun project(configure: (WasmComponentExtension) -> Unit = {}): Pair<org.gradle.api.Project, FakeKspExtension> {
        val project = ProjectBuilder.builder().build()
        val ksp = project.extensions.create("ksp", FakeKspExtension::class.java)
        project.plugins.apply(WasmComponentPlugin::class.java)
        configure(project.extensions.getByType(WasmComponentExtension::class.java))
        (project as ProjectInternal).evaluate()
        return project to ksp
    }

    @Test
    fun `no option is passed to KSP when the headroom is not set`() {
        val (_, ksp) = project()
        assertNull(ksp.args["golem.arenaHeadroomKiB"])
    }

    @Test
    fun `the headroom is passed to KSP as golem arenaHeadroomKiB`() {
        val (_, ksp) = project { it.arenaHeadroomKiB.set(256) }
        assertEquals("256", ksp.args.getValue("golem.arenaHeadroomKiB").get())
    }

    @Test
    fun `a headroom that is not whole 64 KiB pages or is out of range is refused with a message naming the setting`() {
        for (bad in listOf(0, 32, 100, 16448, -64)) {
            val (_, ksp) = project { it.arenaHeadroomKiB.set(bad) }
            val failure = assertFailsWith<Exception>("for $bad") { ksp.args.getValue("golem.arenaHeadroomKiB").get() }
            var cause: Throwable? = failure
            var found = false
            while (cause != null) {
                if ("arenaHeadroomKiB" in (cause.message ?: "")) found = true
                cause = cause.cause
            }
            assertTrue(found, "message should name arenaHeadroomKiB for $bad: ${failure.message}")
        }
    }

    @Test
    fun `the smallest and largest headroom are accepted`() {
        for (ok in listOf(64, 16384)) {
            val (_, ksp) = project { it.arenaHeadroomKiB.set(ok) }
            assertEquals(ok.toString(), ksp.args.getValue("golem.arenaHeadroomKiB").get())
        }
    }

    @Test
    fun `setting the headroom without the KSP plugin fails and says so`() {
        val project = ProjectBuilder.builder().build()
        project.plugins.apply(WasmComponentPlugin::class.java)
        project.extensions.getByType(WasmComponentExtension::class.java).arenaHeadroomKiB.set(256)
        val failure = assertFailsWith<Exception> { (project as ProjectInternal).evaluate() }
        var cause: Throwable? = failure
        var found = false
        while (cause != null) {
            if ("KSP" in (cause.message ?: "")) found = true
            cause = cause.cause
        }
        assertTrue(found, "message should mention KSP: ${failure.message}")
    }

    @Test
    fun `leaving the headroom unset needs no KSP plugin`() {
        val project = ProjectBuilder.builder().build()
        project.plugins.apply(WasmComponentPlugin::class.java)
        (project as ProjectInternal).evaluate()
    }
}
