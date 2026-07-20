package cloud.golem

import cloud.golem.runtime.HostApi
import cloud.golem.runtime.ParseAgentIdResult

// Native (wasmWasi) actuals for BaseAgent's host-backed identity. agentId is wired to the real
// host via golem:api/host@1.5.0's get-self-metadata. agentType is
// wired via golem:agent/host@2.0.0's parse-agent-id. agentName remains unwired:
// see HostApi.ParsedAgentId's doc comment -- there is no well-defined, host-documented way to
// derive it from the WIT-level API (it would require lifting an arbitrary schema-graph).
internal actual fun currentAgentId(): String = HostApi.getSelfMetadata().agentId.agentId

internal actual fun currentAgentType(): String = when (val r = HostApi.parseAgentId(currentAgentId())) {
    is ParseAgentIdResult.Ok -> r.value.agentTypeName
    is ParseAgentIdResult.Err -> ""
}

internal actual fun currentAgentName(): String = ""

// The principal is per-invocation (not host-queryable like agentId): Guest.kt decodes it from the
// `initialize`/`invoke` args and stashes it on NativeAgentRuntime before dispatch.
internal actual fun currentPrincipal(): Principal = cloud.golem.runtime.NativeAgentRuntime.currentPrincipal
