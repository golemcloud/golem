package cloud.golem.gradle

import org.gradle.testfixtures.ProjectBuilder
import java.io.File
import kotlin.test.Test
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/**
 * Input-validation tests for [NativeComponentTask]. Each drives the `componentize` action
 * directly with controlled inputs and asserts the `require` failure -- all of which fire
 * before any wasm-tools invocation, so no external toolchain is needed.
 */
class NativeComponentTaskTest {

    /**
     * Builds a task with the mandatory inputs set and an empty `coreWasmDir`, then lets the
     * caller stage files / override inputs via [configure] (receives the task and its coreWasmDir).
     */
    private fun task(configure: (NativeComponentTask, File) -> Unit): NativeComponentTask {
        val project = ProjectBuilder.builder().build()
        val coreDir = File(project.projectDir, "core").apply { mkdirs() }
        val task = project.tasks.register("nc", NativeComponentTask::class.java).get()
        task.coreWasmDir.set(coreDir)
        task.worldName.set("kotlin-agent")
        task.wasmTools.set("wasm-tools")
        task.outputWasm.set(File(project.projectDir, "out.wasm"))
        configure(task, coreDir)
        return task
    }

    @Test
    fun `fails when no core wasm is present`() {
        // coreWasmDir is left empty -- no *.wasm staged.
        val task = task { _, _ -> }
        val ex = assertFailsWith<IllegalArgumentException> { task.componentize() }
        assertTrue(
            ex.message!!.contains("No compiled Kotlin/Wasm core module"),
            "message was: ${ex.message}",
        )
    }

    @Test
    fun `fails when multiple core wasm files are present`() {
        val task = task { _, coreDir ->
            File(coreDir, "a.wasm").writeText("")
            File(coreDir, "b.wasm").writeText("")
        }
        val ex = assertFailsWith<IllegalArgumentException> { task.componentize() }
        assertTrue(ex.message!!.contains("Expected exactly one"), "message was: ${ex.message}")
    }

    @Test
    fun `fails when the wit-native root has no main wit`() {
        val task = task { task, coreDir ->
            File(coreDir, "core.wasm").writeText("")
            val witDir = File(coreDir.parentFile, "wit").apply { mkdirs() }
            task.witNativeDir.set(witDir.absolutePath)
        }
        val ex = assertFailsWith<IllegalArgumentException> { task.componentize() }
        assertTrue(
            ex.message!!.contains("wit-native root has no main.wit"),
            "message was: ${ex.message}",
        )
    }
}
