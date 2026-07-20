package cloud.golem.gradle

import org.gradle.testfixtures.ProjectBuilder
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

/**
 * Wiring/convention tests for [WasmComponentPlugin], using Gradle's in-process
 * [ProjectBuilder] -- no wasm-tools, no Kotlin/Wasm toolchain, no network.
 */
class WasmComponentPluginTest {

    private fun appliedProject() = ProjectBuilder.builder().build().also { it.plugins.apply(WasmComponentPlugin::class.java) }

    private fun extension(project: org.gradle.api.Project) = project.extensions.getByType(WasmComponentExtension::class.java)

    @Test
    fun `creates the wasmComponent extension`() {
        val ext = appliedProject().extensions.findByName("wasmComponent")
        assertNotNull(ext, "wasmComponent extension should be created")
        assertIs<WasmComponentExtension>(ext)
    }

    @Test
    fun `applies default conventions for worldName and wasmTools`() {
        val ext = extension(appliedProject())
        assertEquals("kotlin-agent", ext.worldName.get())
        assertEquals("wasm-tools", ext.wasmTools.get())
    }

    @Test
    fun `outputWasm defaults from moduleName`() {
        val ext = extension(appliedProject())
        ext.moduleName.set("foo")
        val path = ext.outputWasm.get().asFile.invariantSeparatorsPath
        assertTrue(path.endsWith("build/golem/foo.wasm"), "outputWasm default was: $path")
    }

    @Test
    fun `registers the nativeComponent and wasmComponent tasks in the golem group`() {
        val project = appliedProject()

        val native = project.tasks.getByName("nativeComponent")
        assertIs<NativeComponentTask>(native)
        assertEquals("golem", native.group)

        val alias = project.tasks.getByName("wasmComponent")
        assertEquals("golem", alias.group)
    }

    @Test
    fun `wasmComponent alias depends on nativeComponent`() {
        val project = appliedProject()
        val alias = project.tasks.getByName("wasmComponent")
        val deps = alias.taskDependencies.getDependencies(alias).map { it.name }
        assertTrue("nativeComponent" in deps, "wasmComponent should depend on nativeComponent, got: $deps")
    }
}
