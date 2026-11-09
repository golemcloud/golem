package p25

import cloud.golem.annotations.ToolMiddleware
import cloud.golem.runtime.ToolError
import cloud.golem.runtime.ToolMiddlewareInvocation
import cloud.golem.runtime.ToolMiddlewareOutcome
import cloud.golem.runtime.ToolUnderlying
import cloud.golem.runtime.UniversalToolMiddleware

@ToolMiddleware(name = "kt-pass-through", description = "Forwards the call unchanged")
class PassThrough : UniversalToolMiddleware {
    override fun invoke(invocation: ToolMiddlewareInvocation, underlying: ToolUnderlying): ToolMiddlewareOutcome =
        ToolMiddlewareOutcome.passThrough(underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin))
}

@ToolMiddleware(name = "kt-upper-stdout", description = "Upper-cases the tool's stdout")
class UpperStdout : UniversalToolMiddleware {
    override fun invoke(invocation: ToolMiddlewareInvocation, underlying: ToolUnderlying): ToolMiddlewareOutcome =
        when (val outcome = underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)) {
            is cloud.golem.runtime.UnderlyingOutcome.Ok -> ToolMiddlewareOutcome.Ok(
                outcome.value.result,
                outcome.value.stdout?.let { it.decodeToString().uppercase().encodeToByteArray() },
                outcome.value.stderr,
            )
            else -> ToolMiddlewareOutcome.passThrough(outcome)
        }
}

@ToolMiddleware(name = "kt-deny", description = "Rejects every call")
class Deny : UniversalToolMiddleware {
    override fun invoke(invocation: ToolMiddlewareInvocation, underlying: ToolUnderlying): ToolMiddlewareOutcome =
        ToolMiddlewareOutcome.Err(ToolError.ConstraintViolation("denied by kt-deny: ${invocation.toolName} ${invocation.commandPath}"))
}

data class PrefixItem(val name: String, val enabled: Boolean)

data class PrefixConfig(val text: String, val items: List<PrefixItem>)

@ToolMiddleware(name = "kt-prefix", description = "Prefixes stdout with its typed installation parameters", parameters = PrefixConfig::class)
class Prefix : UniversalToolMiddleware {
    override fun invoke(invocation: ToolMiddlewareInvocation, underlying: ToolUnderlying): ToolMiddlewareOutcome {
        val cfg = invocation.parametersAs<PrefixConfig>()
        val label = "[${cfg.text}:${cfg.items.filter { it.enabled }.joinToString(",") { it.name }}]"
        return when (val outcome = underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin)) {
            is cloud.golem.runtime.UnderlyingOutcome.Ok -> ToolMiddlewareOutcome.Ok(
                outcome.value.result,
                outcome.value.stdout?.let { (label + it.decodeToString()).encodeToByteArray() },
                outcome.value.stderr,
            )
            else -> ToolMiddlewareOutcome.passThrough(outcome)
        }
    }
}
