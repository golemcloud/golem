package cloud.golem.annotations

/**
 * Marks a Kotlin interface as a typed client for a remote agent (agent-to-agent RPC). The
 * interface's abstract methods mirror the remote agent's methods; KSP generates a `<Name>Rpc`
 * implementation that encodes each call's arguments to a `schema-value-tree`, invokes the remote
 * agent via `golem:agent/host`'s wasm-rpc, and decodes the result back to the method's Kotlin
 * return type. [typeName] is the remote agent's registered type name.
 */
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class RemoteAgent(val typeName: String)
