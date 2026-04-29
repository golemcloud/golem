/**
 * Host service for `golem:rdbms/postgres@1.5.0`. Wraps the synchronous
 * `DbConnection.open(address)` static entry point so SDK code can
 * acquire a connection via DI rather than reaching into the imported
 * namespace directly.
 *
 * Per the layer-mocks refactor plan §4c, the `DbConnection`,
 * `DbTransaction`, and `DbResultStream` resources themselves are NOT
 * services — only the constructor / entry point that produces them is.
 * Their methods (`query`, `execute`, `queryStream`, `beginTransaction`,
 * `commit`, `rollback`, …) are still synchronous and called from
 * `src/postgres.ts` inside `Effect.try` wrappers (see the surrounding
 * adapter for error classification).
 *
 * The user-facing `PgClient` adapter (`src/postgres.ts`) keeps its
 * byte-compatible public surface; the only change is that
 * `PgClient.make` / `PgClient.layer` now require {@link PostgresHostClient}
 * to be provided upstream. The agent dispatcher's `provideUserRuntime`
 * helper provides `HostLive` (which contains {@link PostgresHostLive}),
 * so user code inside `defineAgent` impls never sees the requirement.
 *
 * Naming: `PostgresHostClient` (with the `HostClient` suffix) is
 * deliberately distinct from the user-facing `PgClient` adapter that
 * implements `effect/unstable/sql/SqlClient`.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import { DbConnection } from "golem:rdbms/postgres@1.5.0"

export interface PostgresHostClientShape {
  /**
   * Mirrors the static `DbConnection.open(address)` entry point.
   * Synchronous; thrown errors propagate as defects until wrapped by
   * the calling adapter via `Effect.try`.
   */
  readonly open: (address: string) => DbConnection
}

export class PostgresHostClient extends Context.Service<
  PostgresHostClient,
  PostgresHostClientShape
>()("effect-golem/host/Postgres") {}

export const PostgresHostLive: Layer.Layer<PostgresHostClient> = Layer.succeed(
  PostgresHostClient,
  PostgresHostClient.of({
    open: (address) => DbConnection.open(address),
  }),
)
