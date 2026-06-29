package cloud.golem.ksp

import com.google.devtools.ksp.processing.CodeGenerator
import com.google.devtools.ksp.processing.Dependencies

/**
 * Generates `<component>-agent.wit`: a world that includes the Golem agent-guest
 * interface, plus agent-metadata comments tooling (e.g. wit-bindgen-kotlin) can read.
 */
class WitEmitter(private val codeGenerator: CodeGenerator) {

    fun emit(model: AgentModel) {
        val componentName = toKebabCase(
            model.className.removeSuffix("Agent").ifEmpty { model.className }
        )
        val packageName = "kotlin:$componentName"
        val worldName = "$componentName-agent"
        val version = "0.1.0"

        val wit = buildWit(model, packageName, worldName, version)

        codeGenerator.createNewFile(
            dependencies = Dependencies.ALL_FILES,
            packageName = "wit", // KSP treats this as a sub-directory segment
            fileName = worldName,
            extensionName = "wit"
        ).bufferedWriter().use { it.write(wit) }
    }

    private fun buildWit(
        model: AgentModel,
        packageName: String,
        worldName: String,
        version: String
    ): String = buildString {
        appendLine("package $packageName@$version;")
        appendLine()
        appendLine("world $worldName {")
        appendLine("  include golem:agent/agent-guest@2.0.0;")
        appendLine("}")
        appendLine()
        appendLine("// --- Agent metadata (for tooling — not parsed by wasm-tools) ---")
        appendLine("//")
        appendLine("// agent ${model.className} {")
        appendLine("//   mount: \"${sanitizeComment(model.mountPath)}\"")
        appendLine("//   description: \"${sanitizeComment(model.classDescription)}\"")
        if (model.constructorParams.isNotEmpty()) {
            val params = model.constructorParams.joinToString(", ") { "${it.name}: ${it.witType}" }
            appendLine("//   constructor($params)")
        } else {
            appendLine("//   constructor()")
        }
        model.methods.forEach { m ->
            val params = if (m.inputParams.isEmpty()) ""
            else m.inputParams.joinToString(", ") { "${it.name}: ${it.witType}" }
            val returnPart = if (m.outputWitType == "()") "" else " -> ${m.outputWitType}"
            val endpointParts = m.httpEndpoints.joinToString(", ") {
                "${it.verb.lowercase()}: \"${sanitizeComment(it.path)}\""
            }
            val promptPart = if (m.promptHint.isNotEmpty()) ", prompt: \"${sanitizeComment(m.promptHint)}\"" else ""
            val meta = if (endpointParts.isNotEmpty() || promptPart.isNotEmpty())
                " { $endpointParts$promptPart }"
            else ""
            appendLine("//   method ${m.name}($params)$returnPart$meta")
        }
        appendLine("// }")
    }

    /** "CounterAgent" -> "counter", "MyFooAgent" -> "my-foo" */
    private fun toKebabCase(name: String): String =
        name.replace(Regex("([a-z])([A-Z])"), "$1-$2").lowercase()

    // W1: strip embedded newlines from values interpolated into // comment lines
    // so a value containing \n doesn't break the comment into un-commented code.
    private fun sanitizeComment(s: String): String =
        s.replace('\n', ' ').replace('\r', ' ')
}
