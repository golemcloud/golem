/**
 * Public `Durability` namespace barrel.
 *
 * The implementation lives in two internal modules to keep the
 * `golem:api/host@1.5.0` execution-mode controls and the
 * `golem:durability/durability@1.5.0` typed-invocation wrapper
 * decoupled (and to avoid a module-level cycle):
 *
 * - `internal/durabilityMode.ts` — persistence level / idempotence /
 *   atomic regions / oplog-commit / idempotency keys / `unwrapOrRevert`
 *   / `checkpoint` / `compensable`.
 * - `internal/durableFunction.ts` — `wrap` / `wrapInfallible` /
 *   `FunctionType` and the lower-level escape hatches around the
 *   durability host interface.
 *
 * Both are re-exported here so users see a single unified
 * `Durability.*` namespace.
 *
 * @since 0.1.0
 * @category modules
 */
export * from "./internal/durabilityMode.js"
export * from "./internal/durableFunction.js"
