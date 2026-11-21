import { Context, Data, Effect, Layer } from "effect"
import {
  billableOverage,
  BYTE_NANOSECONDS_PER_GB_SECOND,
  type MonthlyPolicyUsage,
  type Period,
  type Usage
} from "./BillableUsage.ts"
import { Database } from "./Database.ts"

/** A failed read of the metering tables of the registry. */
export class MeteringError extends Data.TaggedError("MeteringError")<{
  readonly reason: string
}> {
  override get message(): string {
    return `Reading the metering tables failed: ${this.reason}`
  }
}

export interface MeteringShape {
  /**
   * The billable overage of every account that has usage in `period`, keyed by account ID. The
   * month can be closed: the registry keeps the usage of every month.
   */
  readonly billableOverage: (period: Period) => Effect.Effect<ReadonlyMap<string, Usage>, MeteringError>
}

/**
 * Reads the metering tables of the registry. The database role of the app can read these tables
 * and no other registry table.
 */
export class Metering extends Context.Service<Metering, MeteringShape>()("golem-cloud-billing/Metering") {}

interface RevisionRow {
  readonly account_id: string
  readonly revision: string
  readonly period_year: number
  readonly period_month: number
  readonly mode: string
  readonly baseline_compute_fuel: string
  readonly baseline_memory: string
  readonly baseline_durable_storage: string
  readonly baseline_ephemeral_storage: string
  readonly resolved_compute_fuel: string
  readonly resolved_memory: string
  readonly resolved_durable_storage: string
  readonly resolved_ephemeral_storage: string
  readonly compute_fuel_delta: string
  readonly memory_delta: string
  readonly durable_storage_delta: string
  readonly ephemeral_storage_delta: string
}

/**
 * The usage that each policy revision received in one month, summed over the usage updates. The
 * sums use NUMERIC, so they are exact. Memory and storage combine the whole units and the
 * nanosecond remainders into byte-nanoseconds, and a delta counts only when its meter was on, as
 * `MonthlyPolicyAttributionAggregate` of the registry adds them.
 */
const revisionUsageQuery = (registrySchema: string) => `
  SELECT policy.account_id::text AS account_id,
         policy.revision::text AS revision,
         policy.period_year,
         policy.period_month,
         policy.mode,
         policy.baseline_compute_fuel::text AS baseline_compute_fuel,
         (policy.baseline_memory_gb_seconds * $2::numeric
           + policy.baseline_memory_byte_nanoseconds_remainder)::text AS baseline_memory,
         (policy.baseline_durable_storage_byte_seconds * 1000000000
           + policy.baseline_durable_storage_byte_nanoseconds_remainder)::text AS baseline_durable_storage,
         (policy.baseline_ephemeral_storage_byte_seconds * 1000000000
           + policy.baseline_ephemeral_storage_byte_nanoseconds_remainder)::text AS baseline_ephemeral_storage,
         policy.resolved_compute_fuel::text AS resolved_compute_fuel,
         (policy.resolved_memory_gb_seconds * $2::numeric)::text AS resolved_memory,
         (policy.resolved_durable_storage_byte_seconds * 1000000000)::text AS resolved_durable_storage,
         (policy.resolved_ephemeral_storage_byte_seconds * 1000000000)::text AS resolved_ephemeral_storage,
         sum(CASE WHEN attribution.compute_metering_enabled
               THEN attribution.compute_fuel_delta::numeric ELSE 0 END)::text AS compute_fuel_delta,
         sum(CASE WHEN attribution.memory_metering_enabled
               THEN attribution.memory_gb_seconds_delta::numeric * $2::numeric
                    + attribution.memory_byte_nanoseconds_remainder
               ELSE 0 END)::text AS memory_delta,
         sum(CASE WHEN attribution.filesystem_metering_enabled
               THEN attribution.durable_storage_byte_seconds_delta::numeric * 1000000000
                    + attribution.durable_storage_byte_nanoseconds_remainder
               ELSE 0 END)::text AS durable_storage_delta,
         sum(CASE WHEN attribution.filesystem_metering_enabled
               THEN attribution.ephemeral_storage_byte_seconds_delta::numeric * 1000000000
                    + attribution.ephemeral_storage_byte_nanoseconds_remainder
               ELSE 0 END)::text AS ephemeral_storage_delta
    FROM "${registrySchema}".account_monthly_policy_attribution attribution
    JOIN "${registrySchema}".account_monthly_policy_revisions policy
      ON policy.account_id = attribution.account_id
     AND policy.revision = attribution.revision
   WHERE attribution.usage_key = $1
   GROUP BY policy.account_id, policy.revision`

/** The usage key of the registry for a month: `YYYY-MM`. */
const usageKey = (period: Period): string =>
  `${String(period.year).padStart(4, "0")}-${String(period.month).padStart(2, "0")}`

const toMonthlyPolicyUsage = (row: RevisionRow): MonthlyPolicyUsage => ({
  revision: BigInt(row.revision),
  period: { year: row.period_year, month: row.period_month },
  mode: row.mode === "allow_overage" ? "allowOverage" : "hardLimit",
  baseline: {
    computeFuel: BigInt(row.baseline_compute_fuel),
    memoryByteNanoseconds: BigInt(row.baseline_memory),
    durableStorageByteNanoseconds: BigInt(row.baseline_durable_storage),
    ephemeralStorageByteNanoseconds: BigInt(row.baseline_ephemeral_storage)
  },
  monthlyAmount: {
    computeFuel: BigInt(row.resolved_compute_fuel),
    memoryByteNanoseconds: BigInt(row.resolved_memory),
    durableStorageByteNanoseconds: BigInt(row.resolved_durable_storage),
    ephemeralStorageByteNanoseconds: BigInt(row.resolved_ephemeral_storage)
  },
  delta: {
    computeFuel: BigInt(row.compute_fuel_delta),
    memoryByteNanoseconds: BigInt(row.memory_delta),
    durableStorageByteNanoseconds: BigInt(row.durable_storage_delta),
    ephemeralStorageByteNanoseconds: BigInt(row.ephemeral_storage_delta)
  }
})

export const layer: Layer.Layer<Metering, never, Database> = Layer.effect(
  Metering,
  Effect.gen(function* () {
    const { pool, config } = yield* Database
    const query = revisionUsageQuery(config.registrySchema)

    return {
      billableOverage: (period) =>
        Effect.tryPromise({
          try: () => pool.query<RevisionRow>(query, [usageKey(period), BYTE_NANOSECONDS_PER_GB_SECOND.toString()]),
          catch: (error) => new MeteringError({ reason: error instanceof Error ? error.message : String(error) })
        }).pipe(
          Effect.map((result) => {
            const revisions = new Map<string, Array<MonthlyPolicyUsage>>()
            for (const row of result.rows) {
              const usage = toMonthlyPolicyUsage(row)
              const forAccount = revisions.get(row.account_id)
              if (forAccount === undefined) {
                revisions.set(row.account_id, [usage])
              } else {
                forAccount.push(usage)
              }
            }
            return new Map(
              [...revisions].map(([accountId, forAccount]) => [accountId, billableOverage(forAccount, period)])
            )
          })
        )
    }
  })
)
