/**
 * Runtime mock for `golem:quota/types@1.5.0` used by the test suite. The
 * real host class is provided by the WASM runtime; this just keeps the
 * minimum interface needed by `QuotaToken` round-trips.
 */
export type EnvironmentId = { uuid: { highBits: bigint; lowBits: bigint } }
export type Datetime = { seconds: bigint; nanoseconds: number }

export type QuotaTokenRecord = {
  environmentId: EnvironmentId
  resourceName: string
  expectedUse: bigint
  lastCredit: bigint
  lastCreditAt: Datetime
}

export class QuotaToken {
  private constructor(private readonly record: QuotaTokenRecord) {}

  toRecord(): QuotaTokenRecord {
    return this.record
  }

  static fromRecord(serialized: QuotaTokenRecord): QuotaToken {
    return new QuotaToken(serialized)
  }
}
