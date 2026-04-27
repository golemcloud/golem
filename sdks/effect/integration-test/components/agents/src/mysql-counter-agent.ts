/**
 * MySqlCounter — a durable counter agent whose state lives in a real
 * MySQL database via Golem's `golem:rdbms/mysql@1.5.0` host bindings.
 *
 * Mirrors `pg-counter-agent.ts`: same method surface (value / add /
 * transferAdd / failingAdd / streamAll), same snapshot policy, same
 * `connectionAddress` config field. The actual durable state is in
 * MySQL, so the snapshot schema is `Schema.Struct({})`.
 */
import { Effect, Redacted, Schema } from "effect"
import { defineAgent, defineConfig, method, Snapshot } from "effect-golem"
import { MySqlClient } from "effect-golem/mysql"

export class MySqlCounterConfig extends defineConfig("MySqlCounter.Config", {
  // Distinct from PgCounter's `connectionAddress` so the local
  // `secretDefaults` registry can hold a separate MySQL DSN.
  mysqlConnectionAddress: Schema.Redacted(Schema.String),
}) {}

export const MySqlCounter = defineAgent({
  name: "MySqlCounter",
  description: "A named integer counter backed by MySQL",
  mode: "durable",
  config: MySqlCounterConfig,
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
  impl: ({ name }, snap) =>
    Effect.gen(function* () {
      yield* snap.init({})
      const cfg = yield* MySqlCounterConfig
      const dsnRedacted = yield* cfg.mysqlConnectionAddress.get
      const dsnString = Redacted.value(dsnRedacted)
      const sql = yield* MySqlClient.make({ connectionAddress: dsnString })
      // Idempotent DDL; safe across snapshots and updates. MySQL has
      // no `ON CONFLICT` syntax — use `INSERT IGNORE` instead.
      yield* sql`CREATE TABLE IF NOT EXISTS mysql_counters (id VARCHAR(64) PRIMARY KEY, count INT NOT NULL DEFAULT 0)`
      yield* sql`INSERT IGNORE INTO mysql_counters (id, count) VALUES (${name}, 0)`

      const readCount = (id: string): Effect.Effect<number, unknown> =>
        sql`SELECT count FROM mysql_counters WHERE id = ${id}`.pipe(
          Effect.map((rows) => Number((rows[0] as { count?: number } | undefined)?.count ?? 0)),
        )

      return {
        value: () => readCount(name),
        add: ({ by }) =>
          sql`UPDATE mysql_counters SET count = count + ${by} WHERE id = ${name}`.pipe(
            Effect.flatMap(() => readCount(name)),
          ),
        transferAdd: ({ from, by }) =>
          sql.withTransaction(
            Effect.gen(function* () {
              yield* sql`INSERT IGNORE INTO mysql_counters (id, count) VALUES (${from}, 0)`
              yield* sql`UPDATE mysql_counters SET count = count - ${by} WHERE id = ${from}`
              yield* sql`UPDATE mysql_counters SET count = count + ${by} WHERE id = ${name}`
              return yield* readCount(name)
            }),
          ),
        failingAdd: ({ by }) =>
          sql
            .withTransaction(
              Effect.gen(function* () {
                yield* sql`UPDATE mysql_counters SET count = count + ${by} WHERE id = ${name}`
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
          sql`SELECT id, count FROM mysql_counters ORDER BY id`.pipe(
            Effect.map((rows) =>
              (rows as ReadonlyArray<{ id: string; count: number | bigint }>).map((r) => ({
                id: r.id,
                count: Number(r.count),
              })),
            ),
          ),
      }
    }),
})
