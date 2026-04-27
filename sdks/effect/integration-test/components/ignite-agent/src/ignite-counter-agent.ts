/**
 * IgniteCounter — a durable counter agent whose state lives in a real
 * Apache Ignite 2.x cluster via Golem's `golem:rdbms/ignite2@1.5.0`
 * host bindings.
 *
 * Mirrors `pg-counter-agent.ts` / `mysql-counter-agent.ts` with two
 * Ignite-specific tweaks:
 *
 * - Upserts use `MERGE INTO ...` since Ignite has neither
 *   `INSERT ... ON CONFLICT` nor `INSERT IGNORE`.
 * - There is no nested `withTransaction`: the IgniteClient adapter
 *   explicitly fails on savepoints. The agent's `transferAdd`
 *   transaction is single-level so this constraint does not bite us.
 *
 * This component is deployed separately because the
 * `golem:rdbms/ignite2@1.5.0` host binding may be missing from some
 * Golem environments.
 */
import { Effect, Redacted, Schema } from "effect"
import { defineAgent, defineConfig, method, Snapshot } from "effect-golem"
import { IgniteClient } from "effect-golem/ignite2"

export class IgniteCounterConfig extends defineConfig("IgniteCounter.Config", {
  // Distinct from the other RDBMS counters so the local
  // `secretDefaults` registry can hold an Ignite-specific DSN.
  igniteConnectionAddress: Schema.Redacted(Schema.String),
}) {}

export const IgniteCounter = defineAgent({
  name: "IgniteCounter",
  description: "A named integer counter backed by Apache Ignite",
  mode: "durable",
  config: IgniteCounterConfig,
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
      const cfg = yield* IgniteCounterConfig
      const dsnRedacted = yield* cfg.igniteConnectionAddress.get
      const dsnString = Redacted.value(dsnRedacted)
      const sql = yield* IgniteClient.make({ connectionAddress: dsnString })
      // Idempotent DDL.
      yield* sql`CREATE TABLE IF NOT EXISTS ignite_counters (id VARCHAR PRIMARY KEY, count INT)`
      // Ignite-specific upsert.
      yield* sql`MERGE INTO ignite_counters (id, count) VALUES (${name}, 0)`

      const readCount = (id: string): Effect.Effect<number, unknown> =>
        sql`SELECT count FROM ignite_counters WHERE id = ${id}`.pipe(
          Effect.map((rows) => Number((rows[0] as { count?: number } | undefined)?.count ?? 0)),
        )

      return {
        value: () => readCount(name),
        add: ({ by }) =>
          sql`UPDATE ignite_counters SET count = count + ${by} WHERE id = ${name}`.pipe(
            Effect.flatMap(() => readCount(name)),
          ),
        transferAdd: ({ from, by }) =>
          sql.withTransaction(
            Effect.gen(function* () {
              yield* sql`MERGE INTO ignite_counters (id, count) VALUES (${from}, 0)`
              yield* sql`UPDATE ignite_counters SET count = count - ${by} WHERE id = ${from}`
              yield* sql`UPDATE ignite_counters SET count = count + ${by} WHERE id = ${name}`
              return yield* readCount(name)
            }),
          ),
        failingAdd: ({ by }) =>
          sql
            .withTransaction(
              Effect.gen(function* () {
                yield* sql`UPDATE ignite_counters SET count = count + ${by} WHERE id = ${name}`
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
          sql`SELECT id, count FROM ignite_counters ORDER BY id`.pipe(
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
