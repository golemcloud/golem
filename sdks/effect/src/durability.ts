/**
 * Public `Durability` namespace barrel.
 *
 * The actual implementations live in two files to keep the
 * `golem:api/host@1.5.0` execution-mode controls and the
 * `golem:durability/durability@1.5.0` typed-invocation wrapper
 * decoupled (and to avoid a module-level cycle):
 *
 * - {@link ./durability-mode} — persistence level / idempotence /
 *   atomic regions / oplog-commit / idempotency keys / `unwrapOrRevert`
 *   / `checkpoint` / `compensable`.
 * - {@link ./durable-function} — `wrap` / `wrapInfallible` /
 *   `FunctionType` and the lower-level escape hatches around the
 *   durability host interface.
 *
 * Both are re-exported here so users see a single unified
 * `Durability.*` namespace.
 *
 * @since 0.1.0
 */
/**
 * @since 0.1.0
 * @category re-exports
 */
export * from "./durability-mode.js"
/**
 * @since 0.1.0
 * @category re-exports
 */
export * from "./durable-function.js"
