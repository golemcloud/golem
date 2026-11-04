import { Data, Effect, Scope } from "effect"
import pg from "pg"
import { type MicroUnits, type Period, toMicroUnits } from "./BillableUsage.ts"
import { connectionOptions, Database, withAdvisoryLock } from "./Database.ts"
import { Metering, type MeteringError } from "./Metering.ts"
import { Polar, type PolarError, type PolarEvent } from "./Polar.ts"
import { billableStatuses, tables as subscriptionTables } from "./Subscriptions.ts"

/**
 * The usage push: sends the billable overage of every account to the meters of Polar.
 *
 * Each run reads the billable overage of the previous and the current UTC month, so usage that
 * the registry receives for a month after its last run in that month still reaches Polar. For
 * each account, month and dimension, the table `usage_events` keeps the events of the push. An
 * event goes from the amount that earlier events submitted to the current billable overage, so
 * Polar receives only the increase. A run first records its events and then sends every event
 * that it has not sent yet. Polar skips an event whose external ID it has already, so a run
 * after a failed send sends the same events again and nothing twice.
 *
 * Polar bills an event only while the customer has a billable subscription, and accepts the
 * other events without billing them. So before it sends, a run holds every unsent event of an
 * account without a billable subscription in `polar_subscriptions`, or with one that has passed
 * its scheduled end. A held event is not sent: the warning `unbilled_usage` reports it, and the
 * row keeps it for audit. `Settlement.ts` keeps such usage small.
 */

/** The event names of the meters of Polar, for each dimension. */
export const meterEventNames: Readonly<Record<keyof MicroUnits, string>> = {
  compute: "golem.compute",
  memory: "golem.memory",
  durableStorage: "golem.durable_storage",
  ephemeralStorage: "golem.ephemeral_storage"
}

const dimensions = Object.keys(meterEventNames) as ReadonlyArray<keyof MicroUnits>

export class UsagePushError extends Data.TaggedError("UsagePushError")<{
  readonly reason: string
}> {
  override get message(): string {
    return `The usage push failed: ${this.reason}`
  }
}

export interface PushSummary {
  /** Events that this run recorded. */
  readonly created: number
  /** Events that this run sent, including events of earlier runs that were not sent. */
  readonly sent: number
  /** Events that this run held, because Polar would not bill them. */
  readonly held: number
}

const MICRO_UNITS = 1_000_000
/**
 * Polar refuses a numeric metadata value outside ±(2^53 - 1) with 422 (EventMetadataInput of the
 * event ingestion API), and a larger `micro_units` would also lose precision as a number.
 */
const MAX_EVENT_MICRO_UNITS = BigInt(Number.MAX_SAFE_INTEGER)
const BATCH_SIZE = 100

const periodOf = (date: Date): Period => ({ year: date.getUTCFullYear(), month: date.getUTCMonth() + 1 })

const previous = ({ year, month }: Period): Period => (month === 1 ? { year: year - 1, month: 12 } : { year, month: month - 1 })

const periodKey = (period: Period): string => `${period.year}-${String(period.month).padStart(2, "0")}`

const usageEventsTable = `
  CREATE TABLE IF NOT EXISTS usage_events (
    event_id TEXT PRIMARY KEY,
    account_id UUID NOT NULL,
    period TEXT NOT NULL,
    dimension TEXT NOT NULL,
    from_micro_units NUMERIC NOT NULL,
    to_micro_units NUMERIC NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    sent_at TIMESTAMPTZ,
    held_at TIMESTAMPTZ,
    CONSTRAINT usage_events_sent_or_held CHECK (sent_at IS NULL OR held_at IS NULL)
  );
  CREATE INDEX IF NOT EXISTS usage_events_unsent ON usage_events (created_at)
    WHERE sent_at IS NULL AND held_at IS NULL;
  CREATE INDEX IF NOT EXISTS usage_events_submitted ON usage_events (period, account_id, dimension)`

interface UsageEventRow {
  readonly event_id: string
  readonly account_id: string
  readonly period: string
  readonly dimension: keyof MicroUnits
  readonly from_micro_units: string
  readonly to_micro_units: string
}

const failure = (error: unknown) =>
  new UsagePushError({ reason: error instanceof Error ? error.message : String(error) })

/**
 * Holds a session-level advisory lock until the scope closes, so that two runs do not record the
 * same increase. The lock has its own connection, outside the pool.
 */
const advisoryLock = (options: pg.ClientConfig, key: string): Effect.Effect<void, UsagePushError, Scope.Scope> =>
  Effect.acquireRelease(
    Effect.tryPromise({
      try: async () => {
        const client = new pg.Client(options)
        await client.connect()
        try {
          await client.query("SELECT pg_advisory_lock(hashtextextended($1, 0))", [key])
        } catch (error) {
          await client.end()
          throw error
        }
        return client
      },
      catch: failure
    }),
    (client) => Effect.promise(() => client.end())
  ).pipe(Effect.asVoid)

export const pushUsage = (
  now: Date
): Effect.Effect<PushSummary, UsagePushError | MeteringError | PolarError, Database | Metering | Polar> =>
  Effect.scoped(
    Effect.gen(function* () {
      const { pool, config } = yield* Database
      const metering = yield* Metering
      const polar = yield* Polar
      const query = <R extends pg.QueryResultRow>(text: string, values: ReadonlyArray<unknown> = []) =>
        Effect.tryPromise({ try: () => pool.query<R>(text, [...values]), catch: failure })

      yield* advisoryLock(connectionOptions(config), "golem-cloud-billing/usage-push")
      yield* query(usageEventsTable)
      yield* Effect.tryPromise({
        try: () => withAdvisoryLock(config, "golem-cloud-billing/migrations", () => pool.query(subscriptionTables)),
        catch: failure
      })

      let created = 0
      const current = periodOf(now)
      for (const period of [previous(current), current]) {
        const key = periodKey(period)
        const overage = yield* metering.billableOverage(period)
        const submitted = yield* query<{ account_id: string; dimension: string; submitted: string }>(
          `SELECT account_id::text AS account_id, dimension, max(to_micro_units)::text AS submitted
             FROM usage_events WHERE period = $1 GROUP BY account_id, dimension`,
          [key]
        )
        const submittedBy = new Map(
          submitted.rows.map((row) => [`${row.account_id}:${row.dimension}`, BigInt(row.submitted)])
        )
        for (const [accountId, usage] of overage) {
          const microUnits = toMicroUnits(usage)
          for (const dimension of dimensions) {
            const from = submittedBy.get(`${accountId}:${dimension}`) ?? 0n
            const to = microUnits[dimension]
            // An increase above MAX_EVENT_MICRO_UNITS becomes several events. The ID of each event
            // ends in the amount that it reaches, so a run that repeats the split creates the same IDs.
            for (let start = from; start < to; ) {
              const end = to - start > MAX_EVENT_MICRO_UNITS ? start + MAX_EVENT_MICRO_UNITS : to
              const inserted = yield* query(
                `INSERT INTO usage_events (event_id, account_id, period, dimension, from_micro_units, to_micro_units)
                 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (event_id) DO NOTHING`,
                [`${accountId}:${key}:${dimension}:${end}`, accountId, key, dimension, start.toString(), end.toString()]
              )
              created += inserted.rowCount ?? 0
              start = end
            }
          }
        }
      }

      const held = yield* query<UsageEventRow>(
        `UPDATE usage_events e SET held_at = clock_timestamp()
          WHERE sent_at IS NULL AND held_at IS NULL
            AND NOT EXISTS (
              SELECT 1 FROM polar_subscriptions s
               WHERE s.account_id = e.account_id AND s.status = ANY($1)
                 AND NOT (s.cancel_at_period_end AND coalesce(s.current_period_end <= clock_timestamp(), false)))
          RETURNING event_id, account_id::text AS account_id, period, dimension,
                    from_micro_units::text AS from_micro_units, to_micro_units::text AS to_micro_units`,
        [billableStatuses]
      )
      for (const row of held.rows) {
        yield* Effect.logWarning("Held usage that Polar would not bill").pipe(
          Effect.annotateLogs({
            event: "unbilled_usage",
            golemAccountId: row.account_id,
            period: row.period,
            meter: meterEventNames[row.dimension],
            microUnits: (BigInt(row.to_micro_units) - BigInt(row.from_micro_units)).toString(),
            eventId: row.event_id
          })
        )
      }

      let sent = 0
      for (;;) {
        const unsent = yield* query<UsageEventRow>(
          `SELECT event_id, account_id::text AS account_id, period, dimension,
                  from_micro_units::text AS from_micro_units, to_micro_units::text AS to_micro_units
             FROM usage_events WHERE sent_at IS NULL AND held_at IS NULL
            ORDER BY created_at, event_id LIMIT ${BATCH_SIZE}`
        )
        if (unsent.rows.length === 0) {
          break
        }
        const events = unsent.rows.map((row): PolarEvent => {
          // At most MAX_EVENT_MICRO_UNITS, so the conversion to a number is exact.
          const microUnits = Number(BigInt(row.to_micro_units) - BigInt(row.from_micro_units))
          return {
            name: meterEventNames[row.dimension],
            externalCustomerId: row.account_id,
            externalId: row.event_id,
            metadata: { units: microUnits / MICRO_UNITS, micro_units: microUnits, period: row.period }
          }
        })
        yield* polar.ingestEvents(events)
        yield* query("UPDATE usage_events SET sent_at = clock_timestamp() WHERE event_id = ANY($1)", [
          unsent.rows.map((row) => row.event_id)
        ])
        sent += unsent.rows.length
      }

      return { created, sent, held: held.rows.length }
    })
  )
