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
 */
import { Effect, Redacted, Schema } from "effect"
import { defineAgent, defineConfig, method, Snapshot } from "@golemcloud/effect-golem"
import { IgniteClient } from "@golemcloud/effect-golem/ignite2"
import type { IgniteClient as IgniteClientType } from "@golemcloud/effect-golem/ignite2"

export class IgniteCounterConfig extends defineConfig("IgniteCounter.Config", {
  // Distinct from the other RDBMS counters so the local
  // `secretDefaults` registry can hold an Ignite-specific DSN.
  igniteConnectionAddress: Schema.Redacted(Schema.String),
}) {}

const IgniteCounterSpec = defineAgent({
  name: "IgniteCounter",
  description: "A named integer counter backed by Apache Ignite",
  mode: "durable",
  config: IgniteCounterConfig,
  id: { name: Schema.String },
  snapshotting: Snapshot.define({
    schema: Schema.Struct({}),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    value: method({
      input: {},
      success: Schema.Number,
    }),
    add: method({
      input: { by: Schema.Number },
      success: Schema.Number,
    }),
    transferAdd: method({
      input: { from: Schema.String, by: Schema.Number },
      success: Schema.Number,
    }),
    failingAdd: method({
      input: { by: Schema.Number },
      success: Schema.String,
    }),
    streamAll: method({
      input: {},
      success: Schema.Array(Schema.Struct({ id: Schema.String, count: Schema.Number })),
    }),
  },
})

type IgniteState = {
  readonly name: string
  readonly sql: IgniteClientType
}

const igniteMethods = ({ name, sql }: IgniteState) => {
  const readCount = (id: string): Effect.Effect<number, unknown> =>
    sql`SELECT count FROM ignite_counters WHERE id = ${id}`.pipe(
      Effect.map((rows) => Number((rows[0] as { count?: number } | undefined)?.count ?? 0)),
    )
  return {
    value: () => readCount(name),
    add: ({ by }: { by: number }) =>
      sql`UPDATE ignite_counters SET count = count + ${by} WHERE id = ${name}`.pipe(
        Effect.flatMap(() => readCount(name)),
      ),
    transferAdd: ({ from, by }: { from: string; by: number }) =>
      sql.withTransaction(
        Effect.gen(function* () {
          yield* sql`MERGE INTO ignite_counters (id, count) VALUES (${from}, 0)`
          yield* sql`UPDATE ignite_counters SET count = count - ${by} WHERE id = ${from}`
          yield* sql`UPDATE ignite_counters SET count = count + ${by} WHERE id = ${name}`
          return yield* readCount(name)
        }),
      ),
    failingAdd: ({ by }: { by: number }) =>
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
}

const openIgniteState = (name: string) =>
  Effect.gen(function* () {
    const cfg = yield* IgniteCounterConfig
    const dsnRedacted = yield* cfg.igniteConnectionAddress.get
    const dsnString = Redacted.value(dsnRedacted)
    // Apache Ignite returns column names UPPERCASE by default; the
    // agent code reads `r.count` / `r.id` (lowercase), so plug in a
    // lowercasing result-name transform to bridge the convention gap.
    const sql = yield* IgniteClient.make({
      connectionAddress: dsnString,
      transformResultNames: (s) => s.toLowerCase(),
    })
    return { name, sql }
  })

export const IgniteCounter = IgniteCounterSpec.implement<IgniteState>({
  init: ({ name }) =>
    Effect.gen(function* () {
      const state = yield* openIgniteState(name)
      const { sql } = state
      yield* sql`CREATE TABLE IF NOT EXISTS ignite_counters (id VARCHAR PRIMARY KEY, count INT)`
      yield* sql`MERGE INTO ignite_counters (id, count) VALUES (${name}, 0)`
      return state
    }),
  methods: igniteMethods,
  snapshot: {
    save: () => Effect.succeed({}),
    restore: (_saved, context) => openIgniteState(context.id.name),
  },
})
