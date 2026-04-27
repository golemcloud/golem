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

/**
 * Snapshotting namespace — declarative metadata + per-instance
 * binding helpers for opting in to Golem's snapshot/restore mechanism.
 * See {@link ./snapshot} for the full API.
 */
export * as Snapshot from "./snapshot.js"

export {
  InvalidSnapshotError,
  SnapshotAlreadyBoundError,
  SnapshotNotBoundError,
} from "./snapshot.js"
export type {
  AutoSnapshotBinding,
  AutoSnapshotDef,
  CustomSnapshotBinding,
  CustomSnapshotDef,
  CustomSnapshotHandlers,
  SnapshotBinding,
  SnapshotDef,
  SnapshotPolicy,
} from "./snapshot.js"
export { SnapshotEnvelopeError, UnsupportedSnapshotFormatError } from "./snapshot-envelope.js"

// Mandatory `agent-guest` host exports. Users should not touch these
// directly — they are wired up automatically by `registerAgent`.
export { guest, saveSnapshot, loadSnapshot } from "./exports.js"
