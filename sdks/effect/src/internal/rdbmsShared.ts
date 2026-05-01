/**
 * Internal helpers shared by the `effect-golem/postgres`,
 * `effect-golem/mysql`, and `effect-golem/ignite2` adapters.
 *
 * Not part of the public API — sub-modules consume these via relative
 * imports. Kept deliberately small: only the pieces that genuinely
 * repeat across all three RDBMS host bindings live here.
 *
 * @internal
 * @since 1.5.0
 */
import {
  AuthenticationError,
  ConnectionError,
  SqlError,
  type SqlErrorReason,
  SqlSyntaxError,
  UnknownError,
} from "effect/unstable/sql/SqlError"
import type * as PostgresHost from "golem:rdbms/postgres@1.5.0"
import type * as MysqlHost from "golem:rdbms/mysql@1.5.0"
import type * as IgniteHost from "golem:rdbms/ignite2@1.5.0"

// ---------------------------------------------------------------------------
// Read-vs-write SQL classification
// ---------------------------------------------------------------------------

/**
 * Statements that should be routed to the host's `query(...)` rather
 * than `execute(...)` because they return rows. The regex is shared
 * across adapters; the trailing `RETURNING` clause is dialect-specific
 * and is OR'd in on top of this in {@link isReader}.
 */
export const READ_PREFIX_RE = /^\s*(?:SELECT|WITH|VALUES|SHOW|EXPLAIN|TABLE|DESCRIBE|DESC|CALL)\b/i

/** PostgreSQL / MariaDB style `RETURNING` clause. */
export const RETURNING_RE = /\bRETURNING\b/i

/**
 * Default reader detection. Postgres and MariaDB both honour
 * `RETURNING`; MySQL ≤ 8 and Ignite do not, but the regex is
 * conservative so a stray `RETURNING` simply routes to `query` — the
 * host will respond with the appropriate error.
 */
export const isReader = (sql: string): boolean => READ_PREFIX_RE.test(sql) || RETURNING_RE.test(sql)

// ---------------------------------------------------------------------------
// Param-encoding error
// ---------------------------------------------------------------------------

/**
 * Thrown by per-adapter param encoders when a JS value cannot be
 * mapped to the host's `DbValue`. Wrapped into a {@link SqlError}
 * with a {@link SqlSyntaxError} cause by {@link sqlErrorFor}.
 */
export class ParamEncodingError extends Error {
  constructor(message: string) {
    super(message)
    this.name = "RdbmsParamEncodingError"
  }
}

/**
 * Convert an integral `number | bigint` parameter to a `bigint`
 * without silently dropping precision. JS `number` arguments must be
 * `Number.isSafeInteger(n)` (i.e. `±(2^53 − 1)`-safe); anything else
 * raises a {@link ParamEncodingError} so the caller passes a `bigint`
 * explicitly. `BigInt(n)` itself never sees a non-finite or
 * non-integer number, so we also avoid raw `RangeError`s leaking into
 * `UnknownError` classification.
 */
export const toBigIntChecked = (value: number | bigint, label: string): bigint => {
  if (typeof value === "bigint") return value
  if (!Number.isSafeInteger(value)) {
    throw new ParamEncodingError(`${label} must be a safe integer or bigint; got ${String(value)}`)
  }
  return BigInt(value)
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/**
 * Tag union of every RDBMS host's `Error` variant. The three WIT
 * packages (postgres / mysql / ignite2) are kept in lock-step, so this
 * union is in practice identical for all three — but we derive it
 * from all three to catch lock-step breaks.
 */
type RdbmsHostErrorTag =
  | PostgresHost.Error["tag"]
  | MysqlHost.Error["tag"]
  | IgniteHost.Error["tag"]

/** Canonical set of `Error.tag` values used by every RDBMS WIT package. */
export const RDBMS_ERROR_TAGS: ReadonlySet<RdbmsHostErrorTag> = new Set<RdbmsHostErrorTag>([
  "connection-failure",
  "query-parameter-failure",
  "query-execution-failure",
  "query-response-failure",
  "other",
])

export interface RdbmsHostError {
  readonly tag: RdbmsHostErrorTag
  readonly val?: string
}

const isRdbmsHostError = (e: unknown): e is RdbmsHostError => {
  if (e === null || typeof e !== "object") return false
  const obj = e as { tag?: unknown }
  return typeof obj.tag === "string" && RDBMS_ERROR_TAGS.has(obj.tag as RdbmsHostErrorTag)
}

/**
 * Pull a tagged error out of the various shapes the WIT host bindings
 * can throw: a bare `{tag,val}` object, an `Error` whose `.payload` is
 * tagged, or an `Error` whose `.cause` is tagged.
 */
export const extractTaggedError = (e: unknown): RdbmsHostError | undefined => {
  if (isRdbmsHostError(e)) return e
  if (e instanceof Error) {
    const payload = (e as unknown as { payload?: unknown }).payload
    if (isRdbmsHostError(payload)) return payload
    const cause = (e as unknown as { cause?: unknown }).cause
    if (isRdbmsHostError(cause)) return cause
  }
  return undefined
}

/**
 * Build a `SqlError` factory for one RDBMS adapter. The factory wraps
 * a thrown cause into the appropriate `SqlError` reason based on the
 * tagged-error shape (or falls back to {@link AuthenticationError} /
 * {@link UnknownError} for opaque `Error` instances).
 *
 * `ParamEncodingError` always becomes a {@link SqlSyntaxError} so the
 * caller can distinguish JS-side encoding bugs from server-side
 * failures.
 */
export const sqlErrorFor =
  () =>
  (cause: unknown, message: string, operation: string): SqlError => {
    if (cause instanceof ParamEncodingError) {
      return new SqlError({
        reason: new SqlSyntaxError({
          cause,
          message: `${message}: ${cause.message}`,
          operation,
        }),
      })
    }
    const tagged = extractTaggedError(cause)
    let reason: SqlErrorReason
    if (tagged !== undefined) {
      const detail = tagged.val ?? message
      switch (tagged.tag) {
        case "connection-failure":
          reason = new ConnectionError({ cause, message: detail, operation })
          break
        case "query-parameter-failure":
        case "query-execution-failure":
        case "query-response-failure":
          reason = new SqlSyntaxError({ cause, message: detail, operation })
          break
        case "other":
          reason = new UnknownError({ cause, message: detail, operation })
          break
        default: {
          // Exhaustiveness pin: a new variant on any of the three RDBMS
          // host bindings (postgres / mysql / ignite2) breaks compilation
          // here, naming the missing tag.
          const _exhaustive: never = tagged.tag
          reason = new UnknownError({
            cause,
            message: `unhandled rdbms error tag: ${String(_exhaustive)}`,
            operation,
          })
          break
        }
      }
    } else if (
      cause instanceof Error &&
      /authent|password|role|access denied/i.test(cause.message)
    ) {
      reason = new AuthenticationError({ cause, message: cause.message, operation })
    } else {
      reason = new UnknownError({
        cause,
        message: cause instanceof Error ? cause.message : String(cause),
        operation,
      })
    }
    return new SqlError({ reason })
  }
