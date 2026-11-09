package p29

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.runtime.ToolArgumentInfo
import cloud.golem.runtime.ToolHost
import cloud.golem.runtime.ToolInfo

private fun arg(a: ToolArgumentInfo) = "${a.kind}:${a.name}${if (a.required) "!" else ""}${a.short?.let { "/" + it.toChar() } ?: ""}@${a.typeIndex}"

private fun render(t: ToolInfo): String = buildString {
    append("tool ${t.registration.name} v${t.registration.version} fs=${t.requiresFilesystem} nodes=${t.commands.size}\n")
    for (path in t.runnablePaths()) {
        val c = t.find(path)!!
        val b = c.body!!
        append("  /${path.joinToString("/")} '${c.summary}' aliases=${c.aliases} globals=${c.globals.map(::arg)}\n")
        append("    args=${b.arguments.map(::arg)} stdin=${b.stdin} stdout=${b.stdout} stderr=${b.stderr} result=${b.hasResult} errors=${b.errors} ann=${b.annotations}\n")
    }
}

@Agent(description = "tool reflection spike")
class Inspector(val name: String) : BaseAgent() {
    @Endpoint
    fun all(): String = ToolHost.describeAll().joinToString("") { render(it) }

    @Endpoint
    fun one(tool: String): String = ToolHost.describe(tool)?.let { render(it) } ?: "none"
}
