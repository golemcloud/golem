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
export * from "./self-agent-id.js"
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

/**
 * Retry policy DSL — builders for `Predicate` / `Policy` /
 * `NamedPolicy`, well-known property names, and Effect-typed wrappers
 * around the `golem:api/retry@1.5.0` host bindings (including a scoped
 * helper that integrates with Effect's `Scope`). See {@link ./retry}
 * for the full API.
 */
export * as Retry from "./retry.js"
export { RetryHostError, RetryPolicyValidationError } from "./retry.js"

/**
 * Durability namespace — Effect-typed wrappers around the
 * execution-mode controls on `golem:api/host@1.5.0` (persistence
 * level, idempotence mode, atomic regions, oplog commit,
 * idempotency-key generation). See {@link ./durability} for the full
 * API.
 */
export * as Durability from "./durability.js"
export { DurabilityHostError, DurabilityValidationError } from "./durability.js"

/**
 * Oplog namespace — Effect-typed wrappers around `golem:api/oplog@1.5.0`
 * plus `getOplogIndex` / `setOplogIndex`. See {@link ./oplog} for the
 * full API.
 */
export * as Oplog from "./oplog.js"
export { OplogHostError } from "./oplog.js"

/**
 * Agents namespace — Effect-typed wrappers around the
 * agent-management subset of `golem:api/host@1.5.0` (metadata,
 * fork/revert/update, resolve helpers, the `GetAgents` pager, and the
 * promise rendezvous). See {@link ./agents} for the full API.
 */
export * as Agents from "./agents.js"
export { AgentsHostError, AgentsValidationError, PromiseAlreadyCompletedError } from "./agents.js"

export {
  InvalidSnapshotError,
  SnapshotAlreadyBoundError,
  SnapshotDatabaseDuplicateAttachError,
  SnapshotDatabaseHasAttachmentsError,
  SnapshotDatabaseMissingPartError,
  SnapshotDatabaseNotInAutocommitError,
  SnapshotDatabaseUnknownPartError,
  SnapshotNotBoundError,
} from "./snapshot.js"
export type {
  AttachableDatabase,
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
