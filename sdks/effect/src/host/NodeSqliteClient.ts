/**
 * Host service wrapping the `node:sqlite` `DatabaseSync` constructor.
 *
 * `node:sqlite` is a Node built-in (and is exposed verbatim by Golem's
 * `wasm-rquickjs` runtime), so the only piece we need to abstract is
 * the `new DatabaseSync(filename, opts?)` call: routing it through a
 * tagged service lets tests substitute a fake `DatabaseSync` factory
 * via `Layer.succeed(NodeSqliteClient, …)` instead of monkey-patching
 * the imported namespace.
 *
 * The handle returned by {@link NodeSqliteClientShape.open} is exposed
 * verbatim — its `prepare` / `exec` / etc. methods are still called
 * synchronously by the consumer (`src/sqlite.ts`). Only the
 * acquire-release lifecycle is Effect-typed, so the surrounding scope
 * deterministically closes the handle.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Effect, Layer, Scope } from "effect"
import { DatabaseSync } from "node:sqlite"

/** Subset of `DatabaseSyncOptions` consumed by the SDK. */
export interface NodeSqliteOpenOptions {
  readonly readOnly?: boolean | undefined
}

export interface NodeSqliteClientShape {
  /**
   * Open a fresh `DatabaseSync`. The returned Effect is scoped: when
   * the surrounding scope closes, `db.close()` is called inside an
   * idempotent `try { ... } catch { ... }` block (mirroring the
   * pre-refactor behaviour in `src/sqlite.ts`).
   *
   * Errors thrown by the `DatabaseSync` constructor surface verbatim
   * in the `unknown` error channel; consumers (`src/sqlite.ts`'s
   * `SqliteClient.make`) re-wrap them as `SqlError` at the call site.
   */
  readonly open: (
    filename: string,
    opts?: NodeSqliteOpenOptions,
  ) => Effect.Effect<DatabaseSync, unknown, Scope.Scope>
}

export class NodeSqliteClient extends Context.Service<NodeSqliteClient, NodeSqliteClientShape>()(
  "effect-golem/host/NodeSqlite",
) {}

export const NodeSqliteLive: Layer.Layer<NodeSqliteClient> = Layer.succeed(
  NodeSqliteClient,
  NodeSqliteClient.of({
    open: (filename, opts) =>
      Effect.acquireRelease(
        Effect.try({
          try: () => new DatabaseSync(filename, { readOnly: opts?.readOnly ?? false }),
          catch: (cause) => cause,
        }),
        (db) =>
          Effect.sync(() => {
            try {
              db.close()
            } catch {
              /* idempotent — close() throws if already closed */
            }
          }),
      ),
  }),
)
