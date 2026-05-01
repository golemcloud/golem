/**
 * @since 1.5.0
 */

// ---------------------------------------------------------------------------
// Public namespace exports.
//
// Every public module is surfaced under its PascalCase name as a namespace —
// `Quota.acquireQuotaToken`, `Snapshot.define`, `Http.mount`, etc. This mirrors
// the convention used by the official `effect` / `@effect/*` packages
// (`Effect.map`, `Layer.provide`, `SqlClient.make`, …).
//
// See also `AGENTS.md` ("Module organisation conventions") for the rules every
// new module must follow.
// ---------------------------------------------------------------------------

/**
 * Agent definition machinery. Public surface includes `defineAgent` (also
 * re-exported as a flat alias below), `registerAgent`, and the dispatcher
 * entry points consumed by the generated `agent-guest` shim.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Agent from "./Agent.js"

/**
 * Effect-typed wrappers around the agent-management subset of
 * `golem:api/host@1.5.0` (metadata, fork/revert/update, the `GetAgents`
 * pager, and the promise rendezvous).
 *
 * @since 1.5.0
 * @category modules
 */
export * as Agents from "./Agents.js"

/**
 * Effect-typed wrappers around `wasi:blobstore/*` — container CRUD, object
 * I/O, listing as a `Stream`, and a `forSchema` typed view per container.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Blobstore from "./Blobstore.js"

/**
 * Typed RPC client proxies attached to each `defineAgent` result.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Client from "./Client.js"

/**
 * `defineConfig` (also re-exported as a flat alias below) plus the
 * `ConfigError` plumbing it produces.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Config from "./Config.js"

/**
 * Effect-typed wrappers around the execution-mode controls on
 * `golem:api/host@1.5.0` (persistence level, idempotence mode, atomic
 * regions, oplog commit, idempotency-key generation), plus the high-level
 * `wrap` / `wrapInfallible` durable-function combinator.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Durability from "./Durability.js"

/**
 * `Element` schema for tagged primitives inside `Unstructured` payloads.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Element from "./Element.js"

/**
 * Declarative HTTP routing metadata exposed via `Http.mount` /
 * `Http.endpoint` / verb shorthands (`Http.get` / `Http.post` / …).
 *
 * @since 1.5.0
 * @category modules
 */
export * as Http from "./Http.js"

/**
 * Effect-typed wrappers around the eventually-consistent subset of
 * `wasi:keyvalue@0.1.0` (`eventual` + `eventual-batch`).
 *
 * @since 1.5.0
 * @category modules
 */
export * as KeyValue from "./KeyValue.js"

/**
 * Effect `Logger` backed by `wasi:logging/logging`. The dispatcher installs
 * `Logging.layer` automatically; this module is re-exported for users that
 * want to replace or augment the default wiring.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Logging from "./Logging.js"

/**
 * `method` factory (also re-exported as a flat alias below) plus the
 * method-definition models consumed by `defineAgent`.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Method from "./Method.js"

/**
 * Multipart payloads (text + binary parts) for agent inputs and outputs.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Multimodal from "./Multimodal.js"

/**
 * Effect-typed wrappers around `golem:api/oplog@1.5.0` plus
 * `getOplogIndex` / `setOplogIndex`.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Oplog from "./Oplog.js"

/**
 * Principal identity types attached to agent invocations and snapshots.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Principal from "./Principal.js"

/**
 * Effect-typed wrappers around `golem:quota/types@1.5.0` (`acquireQuotaToken`,
 * `reserve`, `commit`, `withReservation`, `split`, `merge`) plus the Schema
 * codec for sending a `QuotaToken` across an RPC boundary.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Quota from "./Quota.js"

/**
 * Retry-policy DSL — builders for `Predicate` / `Policy` / `NamedPolicy`
 * plus Effect-typed wrappers around `golem:api/retry@1.5.0`.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Retry from "./Retry.js"

/**
 * Effect-idiomatic multi-step transactions on top of the Golem oplog
 * (`withCompensation`, `withFallibleCompensation`, `operation`,
 * `fallibleTransaction`, `infallibleTransaction`).
 *
 * @since 1.5.0
 * @category modules
 */
export * as Saga from "./Saga.js"

/**
 * Accessors for the running agent's own identity (`SelfAgentId`).
 *
 * @since 1.5.0
 * @category modules
 */
export * as SelfAgentId from "./SelfAgentId.js"

/**
 * Snapshot opt-in (`Snapshot.define` / `Snapshot.custom`) and supporting
 * binding / policy / attachable-database types.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Snapshot from "./Snapshot.js"

/**
 * Effect `Tracer` backed by `golem:api/context@1.5.0`. The dispatcher
 * installs `Tracing.layer` automatically; this module is re-exported for
 * users that want to replace or augment the default wiring.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Tracing from "./Tracing.js"

/**
 * Heterogeneous, JSON-like data payloads carried alongside structured agent
 * inputs.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Unstructured from "./Unstructured.js"

/**
 * Effect-typed wrapper around `golem:agent/host@1.5.0.create-webhook`.
 * Bundles `Promises.create` with the host's URL minting and exposes a
 * `Webhook` handle whose `await` Effect resumes when the URL is POSTed to.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Webhook from "./Webhook.js"

/**
 * Effect-typed bridge from the host `golem:websocket/client@1.5.0` resource
 * to the canonical `effect/unstable/socket` `Socket` abstraction.
 *
 * @since 1.5.0
 * @category modules
 */
export * as Websocket from "./Websocket.js"

/**
 * Schema ↔ WIT `wit-value` codec machinery (encoders / decoders / errors).
 *
 * @since 1.5.0
 * @category modules
 */
export * as WitCodec from "./WitCodec.js"

/**
 * Schema ↔ WIT `wit-type` lowering helpers (`Uint8`, `Int32`, …) used to
 * publish agent metadata.
 *
 * @since 1.5.0
 * @category modules
 */
export * as WitTypes from "./WitTypes.js"

// ---------------------------------------------------------------------------
// Flat DSL aliases.
//
// The three user-facing constructors that every agent declaration uses are
// re-exported at the package root. Keeping these flat matches the precedent
// set by `effect`'s `pipe` / `flow` re-exports (kept un-namespaced because
// they are the canonical building blocks) — and matches every existing
// `defineAgent({...})` call site.
//
// This is the *only* exception to the "namespace-only" rule. Every other
// public symbol must be reached through its module namespace.
// ---------------------------------------------------------------------------

/**
 * Declare an agent type. See {@link Agent} for the full surface.
 *
 * @since 1.5.0
 * @category dsl
 */
export { defineAgent } from "./Agent.js"

/**
 * Declare an agent's host-managed configuration. See {@link Config} for the
 * full surface.
 *
 * @since 1.5.0
 * @category dsl
 */
export { defineConfig } from "./Config.js"

/**
 * Declare a method on an agent type. See {@link Method} for the full surface.
 *
 * @since 1.5.0
 * @category dsl
 */
export { method } from "./Method.js"

// ---------------------------------------------------------------------------
// Mandatory `agent-guest` host hooks. These are protocol bindings the
// generated WASM shim imports by name; users should not touch them. Wired up
// automatically by `registerAgent`.
// ---------------------------------------------------------------------------

/**
 * @since 1.5.0
 * @category internal
 */
export { guest, saveSnapshot, loadSnapshot } from "./internal/guest.js"
