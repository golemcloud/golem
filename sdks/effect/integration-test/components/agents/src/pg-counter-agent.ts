/**
 * PgCounter — a durable counter agent whose state lives in a real
 * Postgres database via Golem's `golem:rdbms/postgres@1.5.0` host
 * bindings.
 *
 * The agent declares one config field, `connectionAddress`, which
 * carries the DSN at deploy time (see `golem.yaml` and
 * `secretDefaults`). It exposes:
 *
 * - `value()` — return the current count
 * - `add({ by })` — INSERT-or-UPDATE the row, returning the new count
 * - `transferAdd({ from, by })` — transactionally subtract from one
 *   counter, add to this one, and return the new count
 * - `failingAdd({ by })` — perform a write inside a transaction that
 *   ultimately fails, exercising the rollback path
 * - `streamAll()` — stream every row out of the table and return them
 *   as an array (exercises `executeStream`)
 *
 * Snapshotting is opt-in via `Snapshot.define` so we can drive the
 * snapshot drill in `run-rdbms-tests.mjs` (10 invocations →
 * SNAPSHOT entry → `agent update --await` → re-invoke) without
 * managing a separate state Ref. The actual durable state is in
 * postgres so the schema is `Schema.Struct({})`.
 */
import { Effect, Redacted, Schema } from "effect"
import { defineAgent, defineConfig, method, Snapshot } from "effect-golem"
import { PgClient } from "effect-golem/postgres"

export class PgCounterConfig extends defineConfig("PgCounter.Config", {
  connectionAddress: Schema.Redacted(Schema.String),
}) {}

export const PgCounter = defineAgent({
  name: "PgCounter",
  description: "A named integer counter backed by Postgres",
  mode: "durable",
  config: PgCounterConfig,
  constructorParams: { name: Schema.String },
  snapshot: Snapshot.define({
    schema: Schema.Struct({}),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    value: method({
      params: {},
      success: Schema.Number,
    }),
    add: method({
      params: { by: Schema.Number },
      success: Schema.Number,
    }),
    transferAdd: method({
      params: { from: Schema.String, by: Schema.Number },
      success: Schema.Number,
    }),
    failingAdd: method({
      params: { by: Schema.Number },
      success: Schema.String,
    }),
    streamAll: method({
      params: {},
      success: Schema.Array(Schema.Struct({ id: Schema.String, count: Schema.Number })),
    }),
  },
}).implement(({ name }, snap) =>
  Effect.gen(function* () {
    yield* snap.init({})
    // Resolve the redacted DSN via the agent's config tag.
    const cfg = yield* PgCounterConfig
    const dsnRedacted = yield* cfg.connectionAddress.get
    const dsnString = Redacted.value(dsnRedacted)
    const sql = yield* PgClient.make({ connectionAddress: dsnString })
    // Idempotent DDL; safe across snapshots and updates.
    yield* sql`CREATE TABLE IF NOT EXISTS pg_counters (id text PRIMARY KEY, count integer NOT NULL DEFAULT 0)`
    yield* sql`INSERT INTO pg_counters (id, count) VALUES (${name}, 0) ON CONFLICT (id) DO NOTHING`

    const readCount = (id: string): Effect.Effect<number, unknown> =>
      sql`SELECT count FROM pg_counters WHERE id = ${id}`.pipe(
        Effect.map((rows) => Number((rows[0] as { count?: number } | undefined)?.count ?? 0)),
      )

    return {
      value: () => readCount(name),
      add: ({ by }) =>
        sql`UPDATE pg_counters SET count = count + ${by} WHERE id = ${name}`.pipe(
          Effect.flatMap(() => readCount(name)),
        ),
      transferAdd: ({ from, by }) =>
        sql.withTransaction(
          Effect.gen(function* () {
            yield* sql`INSERT INTO pg_counters (id, count) VALUES (${from}, 0) ON CONFLICT (id) DO NOTHING`
            yield* sql`UPDATE pg_counters SET count = count - ${by} WHERE id = ${from}`
            yield* sql`UPDATE pg_counters SET count = count + ${by} WHERE id = ${name}`
            return yield* readCount(name)
          }),
        ),
      failingAdd: ({ by }) =>
        sql
          .withTransaction(
            Effect.gen(function* () {
              yield* sql`UPDATE pg_counters SET count = count + ${by} WHERE id = ${name}`
              return yield* Effect.fail("forced rollback" as const)
            }),
          )
          .pipe(
            Effect.match({
              onFailure: () => "rolled-back" as const,
              onSuccess: () => "committed" as const,
            }),
          ),
      streamAll: () =>
        sql`SELECT id, count FROM pg_counters ORDER BY id`.pipe(
          Effect.map((rows) =>
            (rows as ReadonlyArray<{ id: string; count: number | bigint }>).map((r) => ({
              id: r.id,
              count: Number(r.count),
            })),
          ),
        ),
    }
  }),
)
