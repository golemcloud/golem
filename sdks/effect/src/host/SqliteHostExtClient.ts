/**
 * Host service wrapping Golem's three `wasm-rquickjs` extensions to
 * Node's built-in `node:sqlite` module:
 *
 * - `serializeDatabaseSync(db) -> Uint8Array`
 * - `restoreDatabaseSync(db, bytes) -> void`
 * - `isAutocommitDatabaseSync(db) -> boolean`
 *
 * These do not exist on stock Node, so the production layer delegates
 * to the same `node:sqlite` namespace import the SDK already used; the
 * `wasm-rquickjs` runtime surfaces them as additional exports on that
 * specifier. Tests substitute fakes via
 * `Layer.succeed(SqliteHostExtClient, …)`.
 *
 * The shape methods are intentionally synchronous (no Effect wrap) so
 * the snapshot dispatcher's existing imperative loops in
 * {@link ../agent.dispatchSaveSnapshot} /
 * {@link ../agent.dispatchLoadSnapshot} can keep their structure: the
 * dispatcher reads the impl out of the user runtime layer once at the
 * top via `Effect.runPromise`, then calls the methods directly inside
 * the synchronous loop. Mirrors the `parseAgentId` / `getSelfMetadata`
 * pattern on {@link AgentHostClient}.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import type { DatabaseSync } from "node:sqlite"
import * as NodeSqlite from "node:sqlite"

export interface SqliteHostExtClientShape {
  /**
   * Mirrors the `serializeDatabaseSync` extension. Returns the raw
   * SQLite file bytes for an autocommit, attachment-free in-memory or
   * filesystem-backed `DatabaseSync`. Errors thrown by the host become
   * Effect defects when wrapped at the call site.
   */
  readonly serializeDatabaseSync: (db: DatabaseSync) => Uint8Array
  /**
   * Mirrors the `restoreDatabaseSync` extension. Overwrites the
   * in-memory state of `db` with the given raw SQLite file bytes.
   */
  readonly restoreDatabaseSync: (db: DatabaseSync, bytes: Uint8Array) => void
  /**
   * Mirrors the `isAutocommitDatabaseSync` extension. Returns true iff
   * `db` is currently in autocommit mode (no open transaction).
   */
  readonly isAutocommitDatabaseSync: (db: DatabaseSync) => boolean
}

export class SqliteHostExtClient extends Context.Service<
  SqliteHostExtClient,
  SqliteHostExtClientShape
>()("effect-golem/host/SqliteHostExt") {}

export const SqliteHostExtLive: Layer.Layer<SqliteHostExtClient> = Layer.succeed(
  SqliteHostExtClient,
  SqliteHostExtClient.of({
    serializeDatabaseSync: (db) => NodeSqlite.serializeDatabaseSync(db),
    restoreDatabaseSync: (db, bytes) => NodeSqlite.restoreDatabaseSync(db, bytes),
    isAutocommitDatabaseSync: (db) => NodeSqlite.isAutocommitDatabaseSync(db),
  }),
)
