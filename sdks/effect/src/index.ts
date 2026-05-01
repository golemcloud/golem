/**
 * @since 0.1.0
 */

/**
 * Re-exports from `./method` — `method` factory and method-definition models.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./method.js"

/**
 * Re-exports from `./agent` — `defineAgent`, `registerAgent`, and the
 * dispatcher entry points.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./agent.js"

/**
 * Re-exports from `./client` — typed RPC client proxies attached to
 * each `defineAgent` result.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./client.js"

/**
 * Re-exports from `./wit-codec` — Schema ↔ WIT `wit-value` codec
 * machinery (encoders / decoders / errors).
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./wit-codec.js"

/**
 * Re-exports from `./wit-types` — Schema ↔ WIT `wit-type` lowering
 * helpers used to publish agent metadata.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./wit-types.js"

/**
 * Re-exports from `./element` — `Element` schema for tagged primitives
 * inside `Unstructured` payloads.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./element.js"

/**
 * Re-exports from `./unstructured` — heterogeneous, JSON-like data
 * payloads carried alongside structured agent inputs.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./unstructured.js"

/**
 * Re-exports from `./multimodal` — multipart payloads (text + binary
 * parts) for agent inputs and outputs.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./multimodal.js"

/**
 * Re-exports from `./principal` — principal identity types attached
 * to agent invocations and snapshots.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./principal.js"

/**
 * Quota namespace — Effect-typed wrappers around `golem:quota/types@1.5.0`
 * (`acquireQuotaToken`, `reserve`, `commit`, `withReservation`,
 * `split`, `merge`) plus the existing Schema codec for sending a
 * `QuotaToken` across an RPC boundary. See {@link ./quota} for the
 * full API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Quota from "./quota.js"

/**
 * Named re-exports from `./quota` — error classes and Schema codecs
 * for the quota module surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
export {
  Datetime,
  EnvironmentId,
  FailedReservationError,
  QuotaHostError,
  QuotaToken,
  QuotaTokenRecord,
  Uuid,
} from "./quota.js"

/**
 * Type-only re-export from `./quota` — opaque scope-managed
 * reservation handle.
 *
 * @since 0.1.0
 * @category re-exports
 */
export type { Reservation } from "./quota.js"

/**
 * Re-exports from `./self-agent-id` — accessors for the running
 * agent's own identity.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./self-agent-id.js"

/**
 * Re-exports from `./config` — `defineConfig` factory and supporting
 * `ConfigError` plumbing.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * from "./config.js"

/**
 * HTTP routes namespace — declarative metadata for exposing agent
 * methods through the Golem host's HTTP server. See {@link ./http} for
 * the full API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Http from "./http.js"

/**
 * Snapshotting namespace — declarative metadata + per-instance
 * binding helpers for opting in to Golem's snapshot/restore mechanism.
 * See {@link ./snapshot} for the full API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Snapshot from "./snapshot.js"

/**
 * Retry policy DSL — builders for `Predicate` / `Policy` /
 * `NamedPolicy`, well-known property names, and Effect-typed wrappers
 * around the `golem:api/retry@1.5.0` host bindings (including a scoped
 * helper that integrates with Effect's `Scope`). See {@link ./retry}
 * for the full API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Retry from "./retry.js"

/**
 * Named re-exports from `./retry` — error classes for the retry
 * module surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { RetryHostError, RetryPolicyValidationError } from "./retry.js"

/**
 * Durability namespace — Effect-typed wrappers around the
 * execution-mode controls on `golem:api/host@1.5.0` (persistence
 * level, idempotence mode, atomic regions, oplog commit,
 * idempotency-key generation). See {@link ./durability} for the full
 * API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Durability from "./durability.js"

/**
 * Named re-exports from `./durability` — error classes for the
 * durability module surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
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
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Oplog from "./oplog.js"

/**
 * Named re-export from `./oplog` — error class for the oplog module
 * surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { OplogHostError } from "./oplog.js"

/**
 * Saga namespace — Effect-idiomatic multi-step transactions on top of
 * the Golem oplog. Provides `withCompensation` (canonical, infallible
 * compensation) + `withFallibleCompensation` (fallible compensation
 * that can drive `FailedAndRolledBackPartially`) + `operation` (paired
 * execute+compensate factory) + `fallibleTransaction` /
 * `infallibleTransaction` entry points. See {@link ./saga} for the
 * full API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Saga from "./saga.js"

/**
 * Named re-export from `./saga` — error class raised when a saga is
 * started inside an already-active saga in the same fiber tree.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { NestedSagaError } from "./saga.js"

/**
 * Type-only re-export from `./saga` — tagged union describing the
 * outcome of a `fallibleTransaction`.
 *
 * @since 0.1.0
 * @category re-exports
 */
export type { TransactionFailure } from "./saga.js"

/**
 * Agents namespace — Effect-typed wrappers around the
 * agent-management subset of `golem:api/host@1.5.0` (metadata,
 * fork/revert/update, resolve helpers, the `GetAgents` pager, and the
 * promise rendezvous). See {@link ./agents} for the full API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Agents from "./agents.js"

/**
 * Named re-exports from `./agents` — error classes for the agents
 * module surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { AgentsHostError, AgentsValidationError, PromiseAlreadyCompletedError } from "./agents.js"

/**
 * Webhook namespace — Effect-typed wrapper around
 * `golem:agent/host@1.5.0.create-webhook`. Bundles `Promises.create`
 * with the host's URL minting and exposes a `Webhook` handle whose
 * `await` Effect resumes when the URL is POSTed to. See
 * {@link ./webhook} for the full API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Webhook from "./webhook.js"

/**
 * Named re-exports from `./webhook` — error classes and the
 * `WebhookPayload` value type surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { WebhookDecodeError, WebhookHostError, WebhookPayload } from "./webhook.js"

/**
 * Type-only re-export from `./webhook` — handle returned by
 * `Webhook.create`.
 *
 * @since 0.1.0
 * @category re-exports
 */
export type { WebhookHandle } from "./webhook.js"

/**
 * Websocket namespace — Effect-typed bridge from the host
 * `golem:websocket/client@1.5.0` resource to the canonical
 * `effect/unstable/socket` `Socket` abstraction. Provides
 * `connect` (scoped Effect), `fromConnection` (lower-level adapter
 * over a custom acquire), `layer` (Layer) and `makeChannel` (duplex
 * Channel). See {@link ./websocket} for the full API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Websocket from "./websocket.js"

/**
 * KeyValue namespace — Effect-typed wrappers around the eventually
 * consistent subset of `wasi:keyvalue@0.1.0` (`eventual` +
 * `eventual-batch`). The host's `atomic` and `cache` interfaces are
 * NOT exposed because they are currently `unimplemented!` in Golem.
 * See {@link ./keyvalue} for the full API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as KeyValue from "./keyvalue.js"

/**
 * Named re-exports from `./keyvalue` — error classes for the
 * key-value module surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { KeyValueDecodeError, KeyValueHostError } from "./keyvalue.js"

/**
 * Type-only re-exports from `./keyvalue` — bucket and schema-typed
 * bucket interfaces.
 *
 * @since 0.1.0
 * @category re-exports
 */
export type { Bucket, SchemaBucket } from "./keyvalue.js"

/**
 * Blobstore namespace — Effect-typed wrappers around
 * `wasi:blobstore/*`. Exposes container CRUD, object I/O (sync byte
 * arrays), object listing as a `Stream`, and a `forSchema` typed
 * view per container. See {@link ./blobstore} for the full API.
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Blobstore from "./blobstore.js"

/**
 * Named re-exports from `./blobstore` — error classes for the
 * blobstore module surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { BlobstoreDecodeError, BlobstoreHostError } from "./blobstore.js"

/**
 * Type-only re-exports from `./blobstore` — container, metadata,
 * object identifier, and schema-typed container interfaces.
 *
 * @since 0.1.0
 * @category re-exports
 */
export type {
  ByteRange,
  Container,
  ContainerMetadata,
  ObjectId,
  ObjectMetadata,
  SchemaContainer,
} from "./blobstore.js"

/**
 * Logging namespace — Effect `Logger` backed by `wasi:logging/logging`.
 * The agent dispatcher installs `Logging.layer` automatically so every
 * `Effect.log*` call is forwarded to the Golem host's structured log
 * sink. See {@link ./logging} for the full API (custom layer wiring,
 * imperative `log`, level mapping, safe stringification).
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Logging from "./logging.js"

/**
 * Named re-export from `./logging` — error class for the logging
 * module surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { LoggingHostError } from "./logging.js"

/**
 * Tracing namespace — Effect `Tracer` backed by `golem:api/context@1.5.0`.
 * The agent dispatcher installs `Tracing.layer` automatically and
 * chains the in-Effect span tree under the host's invocation context,
 * so every `Effect.withSpan` becomes a child span of the live host
 * invocation. See {@link ./tracing} for the full API (custom layer
 * wiring, current-context snapshots, header-forwarding controls).
 *
 * @since 0.1.0
 * @category re-exports
 */
export * as Tracing from "./tracing.js"

/**
 * Named re-export from `./tracing` — error class for the tracing
 * module surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { TracingHostError } from "./tracing.js"

/**
 * Named re-exports from `./snapshot` — error classes for the
 * snapshotting module surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
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

/**
 * Type-only re-exports from `./snapshot` — snapshot definition,
 * binding, policy, and attachable-database interfaces.
 *
 * @since 0.1.0
 * @category re-exports
 */
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

/**
 * Named re-exports from `./snapshot-envelope` — error classes for the
 * snapshot envelope codec surfaced at the package root.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { SnapshotEnvelopeError, UnsupportedSnapshotFormatError } from "./snapshot-envelope.js"

// Mandatory `agent-guest` host exports. Users should not touch these
// directly — they are wired up automatically by `registerAgent`.
/**
 * Re-exports from `./exports` — mandatory `agent-guest` host hooks
 * (`guest`, `saveSnapshot`, `loadSnapshot`). Users should not touch
 * these directly; they are wired up automatically by `registerAgent`.
 *
 * @since 0.1.0
 * @category re-exports
 */
export { guest, saveSnapshot, loadSnapshot } from "./exports.js"
