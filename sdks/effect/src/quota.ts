import { Schema, SchemaGetter } from "effect"
import * as Quota from "golem:quota/types@1.5.0"
import { Int64, Uint32, Uint64 } from "./wit-types.js"

/**
 * Schema for `golem:core/types@1.5.0`.Uuid — a 128-bit value carried as
 * two 64-bit halves.
 */
export const Uuid = Schema.Struct({
  highBits: Uint64,
  lowBits: Uint64,
})

/** Schema for `golem:api/host@1.5.0`.EnvironmentId. */
export const EnvironmentId = Schema.Struct({
  uuid: Uuid,
})

/** Schema for `wasi:clocks/wall-clock@0.2.3`.Datetime. */
export const Datetime = Schema.Struct({
  seconds: Int64,
  nanoseconds: Uint32,
})

/**
 * Schema for the wire shape of a `QuotaToken` — the record returned by
 * `QuotaToken.toRecord()` and accepted by `QuotaToken.fromRecord()`.
 */
export const QuotaTokenRecord = Schema.Struct({
  environmentId: EnvironmentId,
  resourceName: Schema.String,
  expectedUse: Uint64,
  lastCredit: Int64,
  lastCreditAt: Datetime,
})

/**
 * Schema for the host `QuotaToken` class. The decoded `Type` is the
 * runtime `QuotaToken` instance; the `Encoded` form is `QuotaTokenRecord`,
 * which the codec then maps onto the corresponding WIT record.
 *
 * Bridging is done by `QuotaToken.fromRecord` / `QuotaToken.toRecord` —
 * the official host class invariants are preserved end-to-end.
 */
export const QuotaToken: Schema.Codec<
  Quota.QuotaToken,
  typeof QuotaTokenRecord.Encoded,
  never,
  never
> = QuotaTokenRecord.pipe(
  Schema.decodeTo(
    Schema.declare((u): u is Quota.QuotaToken => u instanceof Quota.QuotaToken),
    {
      decode: SchemaGetter.transform((rec: typeof QuotaTokenRecord.Type) =>
        Quota.QuotaToken.fromRecord(rec),
      ),
      encode: SchemaGetter.transform((tok: Quota.QuotaToken) => tok.toRecord()),
    },
  ),
)
