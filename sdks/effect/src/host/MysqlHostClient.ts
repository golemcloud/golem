/**
 * Host service for `golem:rdbms/mysql@1.5.0`. Wraps the synchronous
 * `DbConnection.open(address)` static entry point so SDK code can
 * acquire a connection via DI rather than reaching into the imported
 * namespace directly.
 *
 * Per the layer-mocks refactor plan §4c, the `DbConnection`,
 * `DbTransaction`, and `DbResultStream` resources themselves are NOT
 * services — only the constructor / entry point that produces them is.
 * Their methods (`query`, `execute`, `queryStream`, `beginTransaction`,
 * `commit`, `rollback`, …) are still synchronous and called from
 * `src/mysql.ts` inside `Effect.try` wrappers (see the surrounding
 * adapter for error classification).
 *
 * The user-facing `MySqlClient` adapter (`src/mysql.ts`) keeps its
 * byte-compatible public surface; the only change is that
 * `MySqlClient.make` / `MySqlClient.layer` now require
 * {@link MysqlHostClient} to be provided upstream. The agent
 * dispatcher's `provideUserRuntime` helper provides `HostLive` (which
 * contains {@link MysqlHostLive}), so user code inside `defineAgent`
 * impls never sees the requirement.
 *
 * Naming: `MysqlHostClient` (with the `HostClient` suffix) is
 * deliberately distinct from the user-facing `MySqlClient` adapter
 * that implements `effect/unstable/sql/SqlClient`.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import { DbConnection } from "golem:rdbms/mysql@1.5.0"

export interface MysqlHostClientShape {
  /**
   * Mirrors the static `DbConnection.open(address)` entry point.
   * Synchronous; thrown errors propagate as defects until wrapped by
   * the calling adapter via `Effect.try`.
   */
  readonly open: (address: string) => DbConnection
}

export class MysqlHostClient extends Context.Service<MysqlHostClient, MysqlHostClientShape>()(
  "effect-golem/host/Mysql",
) {}

export const MysqlHostLive: Layer.Layer<MysqlHostClient> = Layer.succeed(
  MysqlHostClient,
  MysqlHostClient.of({
    open: (address) => DbConnection.open(address),
  }),
)
