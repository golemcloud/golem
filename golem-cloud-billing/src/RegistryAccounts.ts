import { Context, Data, Effect, Layer } from "effect"
import { Database } from "./Database.ts"

/** A failed read of the account tables of the registry. */
export class RegistryAccountsError extends Data.TaggedError("RegistryAccountsError")<{
  readonly reason: string
}> {
  override get message(): string {
    return `Reading the account tables of the registry failed: ${this.reason}`
  }
}

export interface RegistryAccountsShape {
  /**
   * Whether the registry deleted the account. The registry answers 404 both for a deleted account
   * and for an account that the billing account cannot see, so only this read tells them apart.
   * An account without a row is not deleted.
   */
  readonly isDeleted: (accountId: string) => Effect.Effect<boolean, RegistryAccountsError>
  /** The accounts among `accountIds` that the registry deleted. */
  readonly deletedAmong: (
    accountIds: ReadonlyArray<string>
  ) => Effect.Effect<ReadonlyArray<string>, RegistryAccountsError>
}

/**
 * Reads two columns of the registry table `accounts`: `account_id` and `deleted_at`. The database
 * role of the app has a column grant on these columns and on no other column of the table, which
 * holds the email.
 */
export class RegistryAccounts extends Context.Service<RegistryAccounts, RegistryAccountsShape>()(
  "golem-cloud-billing/RegistryAccounts"
) {}

const failure = (error: unknown) =>
  new RegistryAccountsError({ reason: error instanceof Error ? error.message : String(error) })

export const layer: Layer.Layer<RegistryAccounts, never, Database> = Layer.effect(
  RegistryAccounts,
  Effect.gen(function* () {
    const { pool, config } = yield* Database
    const registry = `"${config.registrySchema}"`
    const query = <R extends object>(text: string, values: ReadonlyArray<unknown>) =>
      Effect.tryPromise({ try: () => pool.query<R & Record<string, unknown>>(text, [...values]), catch: failure })

    const isDeleted = (accountId: string) =>
      query<{ deleted: boolean }>(
        `SELECT EXISTS (SELECT 1 FROM ${registry}.accounts WHERE account_id = $1 AND deleted_at IS NOT NULL) AS deleted`,
        [accountId]
      ).pipe(Effect.map((result) => result.rows[0]?.deleted === true))

    const deletedAmong = (accountIds: ReadonlyArray<string>) =>
      accountIds.length === 0
        ? Effect.succeed([])
        : query<{ account_id: string }>(
            `SELECT account_id::text AS account_id FROM ${registry}.accounts
              WHERE account_id = ANY($1::uuid[]) AND deleted_at IS NOT NULL`,
            [accountIds]
          ).pipe(Effect.map((result) => result.rows.map((row) => row.account_id)))

    return { isDeleted, deletedAmong }
  })
)
