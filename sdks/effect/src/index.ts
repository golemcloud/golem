// User-facing API for declaring agents and methods on top of Effect Schema.
export * from "./method.js"
export * from "./agent.js"
export * from "./client.js"
export * from "./wit-codec.js"
export * from "./wit-types.js"
export * from "./element.js"
export * from "./unstructured.js"
export * from "./multimodal.js"
export * from "./quota.js"
export * from "./principal.js"
export * from "./config.js"

/**
 * HTTP routes namespace — declarative metadata for exposing agent
 * methods through the Golem host's HTTP server. See {@link ./http} for
 * the full API.
 */
export * as Http from "./http.js"

// Mandatory `agent-guest` host exports. Users should not touch these
// directly — they are wired up automatically by `registerAgent`.
export { guest, saveSnapshot, loadSnapshot } from "./exports.js"
