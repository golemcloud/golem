package cloud.golem.runtime

import kotlin.js.JsModule

/**
 * Namespace binding for the baked-in `golem:api/host@1.5.0` host module.
 *
 * This is the A-Q2 mechanism: wasm-rquickjs registers each WIT host interface as
 * an importable JS module under its interface name, with camelCased functions
 * (`get-self-metadata` -> `getSelfMetadata`). Declaring it as an `external object`
 * makes Kotlin/JS emit a namespace import (`import * as ... from 'golem:api/host@1.5.0'`)
 * that rollup keeps external and QuickJS resolves at runtime.
 *
 * MUST stay in its own file: Kotlin forbids a file containing a @JsModule
 * declaration from also holding non-external declarations.
 */
@JsModule("golem:api/host@1.5.0")
external object GolemApiHost {
    /** Returns the current agent's metadata (golem:api/host agent-metadata record). */
    fun getSelfMetadata(): dynamic
}
