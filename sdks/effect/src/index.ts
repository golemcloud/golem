// User-facing API for declaring agents and methods on top of Effect Schema.
export * from "./method.js"
export * from "./agent.js"
export * from "./client.js"
export * from "./wit-codec.js"
export * from "./wit-types.js"
export * from "./element.js"
export * from "./unstructured.js"
export * from "./multimodal.js"
export * from "./principal.js"

/**
 * Quota namespace — Effect-typed wrappers around `golem:quota/types@1.5.0`
 * (`acquireQuotaToken`, `reserve`, `commit`, `withReservation`,
 * `split`, `merge`) plus the existing Schema codec for sending a
 * `QuotaToken` across an RPC boundary. See {@link ./quota} for the
 * full API.
 */
export * as Quota from "./quota.js"
export {
  Datetime,
  EnvironmentId,
  FailedReservationError,
  QuotaHostError,
  QuotaToken,
  QuotaTokenRecord,
  Uuid,
} from "./quota.js"
export type { Reservation } from "./quota.js"
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
export {
  DurabilityDecodeError,
  DurabilityHostError,
  DurabilityReplayMismatchError,
  DurabilityValidationError,
  NestedDurableFunctionError,
} from "./durability.js"

/**
 * Oplog namespace — Effect-typed wrappers around `golem:api/oplog@1.5.0`
 * plus `getOplogIndex` / `setOplogIndex`. See {@link ./oplog} for the
 * full API.
 */
export * as Oplog from "./oplog.js"
export { OplogHostError } from "./oplog.js"

/**
 * Saga namespace — Effect-idiomatic multi-step transactions on top of
 * the Golem oplog. Provides `withCompensation` (canonical, infallible
 * compensation) + `withFallibleCompensation` (fallible compensation
 * that can drive `FailedAndRolledBackPartially`) + `operation` (paired
 * execute+compensate factory) + `fallibleTransaction` /
 * `infallibleTransaction` entry points. See {@link ./saga} for the
 * full API.
 */
export * as Saga from "./saga.js"
export { NestedSagaError } from "./saga.js"
export type { TransactionFailure } from "./saga.js"

/**
 * Agents namespace — Effect-typed wrappers around the
 * agent-management subset of `golem:api/host@1.5.0` (metadata,
 * fork/revert/update, resolve helpers, the `GetAgents` pager, and the
 * promise rendezvous). See {@link ./agents} for the full API.
 */
export * as Agents from "./agents.js"
export { AgentsHostError, AgentsValidationError, PromiseAlreadyCompletedError } from "./agents.js"

/**
 * Webhook namespace — Effect-typed wrapper around
 * `golem:agent/host@1.5.0.create-webhook`. Bundles `Promises.create`
 * with the host's URL minting and exposes a `Webhook` handle whose
 * `await` Effect resumes when the URL is POSTed to. See
 * {@link ./webhook} for the full API.
 */
export * as Webhook from "./webhook.js"
export { WebhookDecodeError, WebhookHostError, WebhookPayload } from "./webhook.js"
export type { WebhookHandle } from "./webhook.js"

/**
 * Logging namespace — Effect `Logger` backed by `wasi:logging/logging`.
 * The agent dispatcher installs `Logging.layer` automatically so every
 * `Effect.log*` call is forwarded to the Golem host's structured log
 * sink. See {@link ./logging} for the full API (custom layer wiring,
 * imperative `log`, level mapping, safe stringification).
 */
export * as Logging from "./logging.js"
export { LoggingHostError } from "./logging.js"

/**
 * Tracing namespace — Effect `Tracer` backed by `golem:api/context@1.5.0`.
 * The agent dispatcher installs `Tracing.layer` automatically and
 * chains the in-Effect span tree under the host's invocation context,
 * so every `Effect.withSpan` becomes a child span of the live host
 * invocation. See {@link ./tracing} for the full API (custom layer
 * wiring, current-context snapshots, header-forwarding controls).
 */
export * as Tracing from "./tracing.js"
export { TracingHostError } from "./tracing.js"

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
