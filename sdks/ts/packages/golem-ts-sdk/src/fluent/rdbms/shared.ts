// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Shared, host-import-free internals for the fluent RDBMS surfaces
// (`postgres.ts`, `mysql.ts`, `ignite.ts`). Ported from effect-golem's
// `internal/rdbmsShared.ts` plus the per-driver `internal/codec.ts`, with the
// Effect machinery removed: `Effect.try` becomes a `wrap(op, fn)` helper that
// throws a typed {@link RdbmsError}, and the SqlClient error classification is
// folded into the same error type.
//
// IMPORTANT: this file MUST NOT import any `golem:rdbms/*` host binding at the
// top level — only the type-only `golem:rdbms/types@1.5.0` shared structs. The
// host resources (`DbConnection`, …) are WASM-only and would break node/vitest
// resolution. The codecs below are typed against structural mirrors of each
// driver's `DbValue` union (which are byte-compatible with the host's jco
// types) so they round-trip in plain node with no host present.

import type { IpAddress, MacAddress, Timestamp, Timestamptz } from 'golem:rdbms/types@1.5.0';

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/** Canonical set of `Error.tag` values used by every RDBMS WIT package. */
const RDBMS_ERROR_TAGS: ReadonlySet<string> = new Set([
  'connection-failure',
  'query-parameter-failure',
  'query-execution-failure',
  'query-response-failure',
  'other',
]);

interface RdbmsHostError {
  readonly tag: string;
  readonly val?: string;
}

const isRdbmsHostError = (e: unknown): e is RdbmsHostError => {
  if (e === null || typeof e !== 'object') return false;
  const obj = e as { tag?: unknown };
  return typeof obj.tag === 'string' && RDBMS_ERROR_TAGS.has(obj.tag);
};

/**
 * Pull a tagged error out of the various shapes the WIT host bindings can
 * throw: a bare `{tag,val}` object, an `Error` whose `.payload` is tagged, or
 * an `Error` whose `.cause` is tagged.
 */
const extractTaggedError = (e: unknown): RdbmsHostError | undefined => {
  if (isRdbmsHostError(e)) return e;
  if (e instanceof Error) {
    const payload = (e as unknown as { payload?: unknown }).payload;
    if (isRdbmsHostError(payload)) return payload;
    const cause = (e as unknown as { cause?: unknown }).cause;
    if (isRdbmsHostError(cause)) return cause;
  }
  return undefined;
};

/**
 * Coarse classification of an RDBMS failure, derived from the host's tagged
 * `Error` variant (or from a JS-side {@link ParamEncodingError}). Mirrors the
 * SqlError reasons effect-golem produced, collapsed onto a plain string union
 * so callers can branch without pulling in Effect.
 */
export type RdbmsErrorReason =
  | 'param-encoding'
  | 'connection-failure'
  | 'query-parameter-failure'
  | 'query-execution-failure'
  | 'query-response-failure'
  | 'authentication'
  | 'other';

const traceOf = (e: unknown): string => {
  if (e !== null && typeof e === 'object') {
    const obj = e as { trace?: () => string; message?: string; val?: unknown };
    if (typeof obj.trace === 'function') {
      try {
        return obj.trace();
      } catch {
        /* fall through */
      }
    }
    if (typeof obj.val === 'string') return obj.val;
    if (typeof obj.message === 'string') return obj.message;
  }
  return String(e);
};

/**
 * Thrown by any fluent RDBMS operation when a host call traps, or when a JS
 * parameter cannot be encoded to the host's `DbValue`. The {@link reason}
 * carries the coarse classification; {@link trace} is the verbatim
 * driver-supplied detail string (opaque / driver-specific — do not parse).
 *
 * Driver modules re-export thin subclasses (`PostgresError`, `MySqlError`,
 * `IgniteError`) so callers can `instanceof`-narrow by driver.
 */
export class RdbmsError extends Error {
  override readonly name: string = 'RdbmsError';
  readonly reason: RdbmsErrorReason;
  readonly trace: string;
  readonly operation: string;
  constructor(
    readonly cause: unknown,
    operation: string,
    driver: string,
  ) {
    const { reason, detail } = classify(cause);
    super(`${driver}Error(${operation}, ${reason}): ${detail}`);
    this.reason = reason;
    this.trace = detail;
    this.operation = operation;
  }
}

const classify = (cause: unknown): { reason: RdbmsErrorReason; detail: string } => {
  if (cause instanceof ParamEncodingError) {
    return { reason: 'param-encoding', detail: cause.message };
  }
  const tagged = extractTaggedError(cause);
  if (tagged !== undefined) {
    const detail = tagged.val ?? traceOf(cause);
    switch (tagged.tag) {
      case 'connection-failure':
        return { reason: 'connection-failure', detail };
      case 'query-parameter-failure':
        return { reason: 'query-parameter-failure', detail };
      case 'query-execution-failure':
        return { reason: 'query-execution-failure', detail };
      case 'query-response-failure':
        return { reason: 'query-response-failure', detail };
      default:
        return { reason: 'other', detail };
    }
  }
  if (cause instanceof Error && /authent|password|role|access denied/i.test(cause.message)) {
    return { reason: 'authentication', detail: cause.message };
  }
  return { reason: 'other', detail: traceOf(cause) };
};

/**
 * Build a `wrap(op, fn)` helper for one driver. Synchronous host calls run
 * inside `fn`; any throw is re-wrapped into a driver-typed {@link RdbmsError}
 * (unless it already is one). Returns `fn`'s value on success.
 */
export const makeWrap =
  (driver: string, ErrorCtor: new (cause: unknown, op: string, driver: string) => RdbmsError) =>
  <A>(operation: string, fn: () => A): A => {
    try {
      return fn();
    } catch (cause) {
      if (cause instanceof RdbmsError) throw cause;
      throw new ErrorCtor(cause, operation, driver);
    }
  };

// ---------------------------------------------------------------------------
// Param-encoding error
// ---------------------------------------------------------------------------

/**
 * Thrown by the per-driver param encoders when a JS value cannot be mapped to
 * the host's `DbValue`. Surfaced to callers as an {@link RdbmsError} with
 * `reason: "param-encoding"`.
 */
export class ParamEncodingError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'RdbmsParamEncodingError';
  }
}

/**
 * Convert an integral `number | bigint` to a `bigint` without silently dropping
 * precision. JS `number` arguments must be `Number.isSafeInteger`-safe;
 * anything else raises a {@link ParamEncodingError}.
 */
export const toBigIntChecked = (value: number | bigint, label: string): bigint => {
  if (typeof value === 'bigint') return value;
  if (!Number.isSafeInteger(value)) {
    throw new ParamEncodingError(`${label} must be a safe integer or bigint; got ${String(value)}`);
  }
  return BigInt(value);
};

// ---------------------------------------------------------------------------
// Read-vs-write SQL classification
// ---------------------------------------------------------------------------

const READ_PREFIX_RE = /^\s*(?:SELECT|WITH|VALUES|SHOW|EXPLAIN|TABLE|DESCRIBE|DESC|CALL)\b/i;
const RETURNING_RE = /\bRETURNING\b/i;

/**
 * Heuristic: does `sql` return rows (→ host `query`) or not (→ host `execute`)?
 * Conservative — a stray `RETURNING` simply routes to `query` and the host
 * responds with the appropriate error.
 */
export const isReader = (sql: string): boolean => READ_PREFIX_RE.test(sql) || RETURNING_RE.test(sql);

// ---------------------------------------------------------------------------
// Temporal decode mode
// ---------------------------------------------------------------------------

/**
 * How temporal values (timestamp/timestamptz/date/time) are decoded from rows.
 *
 * - `"raw"` (default): keep the raw struct from the host as-is. No precision
 *   loss, but consumers must pattern-match on the struct shape.
 * - `"date"`: decode `timestamp` / `timestamptz` / `date` to a JS `Date`
 *   (UTC). `time` / `timetz` stay raw because `Date` cannot hold them.
 */
export type TemporalDecodeMode = 'raw' | 'date';

// ---------------------------------------------------------------------------
// Numeric / null constants (shared across drivers)
// ---------------------------------------------------------------------------

const I32_MIN = -2_147_483_648;
const I32_MAX = 2_147_483_647;
const I64_MIN = BigInt('-9223372036854775808');
const I64_MAX = BigInt('9223372036854775807');
const U64_MAX = BigInt('18446744073709551615');

const isSafeInt32 = (n: number): boolean => Number.isInteger(n) && n >= I32_MIN && n <= I32_MAX;

const checkInt64 = (b: bigint): void => {
  if (b < I64_MIN || b > I64_MAX) {
    throw new ParamEncodingError(`bigint ${b.toString()} is out of int64 range`);
  }
};

const checkUint64 = (b: bigint, label: string): void => {
  if (b < 0n || b > U64_MAX) {
    throw new ParamEncodingError(`${label} ${b.toString()} is out of u64 range`);
  }
};

// ---------------------------------------------------------------------------
// Structural mirrors of the host metadata shapes (host-import-free)
// ---------------------------------------------------------------------------

/** A `LazyDbValue`-shaped wrapper: `{ get(): DbValue }`. */
interface Lazy<T> {
  get(): T;
}

interface Uuid {
  highBits: bigint;
  lowBits: bigint;
}

/** Generic row/column shapes shared by all three drivers. */
export interface DbRowLike<V> {
  values: V[];
}
export interface DbColumnLike {
  name: string;
}

// ---------------------------------------------------------------------------
// Shared temporal helpers
// ---------------------------------------------------------------------------

const dateToTimestamp = (d: Date): Timestamp => {
  const ms = d.getTime();
  if (!Number.isFinite(ms)) {
    throw new ParamEncodingError('Date is not a valid timestamp');
  }
  return {
    date: { year: d.getUTCFullYear(), month: d.getUTCMonth() + 1, day: d.getUTCDate() },
    time: {
      hour: d.getUTCHours(),
      minute: d.getUTCMinutes(),
      second: d.getUTCSeconds(),
      nanosecond: d.getUTCMilliseconds() * 1_000_000,
    },
  };
};

const dateToTimestamptz = (d: Date): Timestamptz => ({ timestamp: dateToTimestamp(d), offset: 0 });

const timestampToDate = (ts: Timestamp): Date => {
  const ms = Math.floor(ts.time.nanosecond / 1_000_000);
  return new Date(
    Date.UTC(
      ts.date.year,
      ts.date.month - 1,
      ts.date.day,
      ts.time.hour,
      ts.time.minute,
      ts.time.second,
      ms,
    ),
  );
};

const timestamptzToDate = (tstz: Timestamptz): Date => {
  // Offset is seconds east of UTC; subtract to recover the UTC instant.
  const baseMs = timestampToDate(tstz.timestamp).getTime();
  return new Date(baseMs - tstz.offset * 1000);
};

const dateOnlyToDate = (d: { year: number; month: number; day: number }): Date =>
  new Date(Date.UTC(d.year, d.month - 1, d.day));

const uuidToString = (uuid: Uuid): string => {
  const hi = uuid.highBits.toString(16).padStart(16, '0');
  const lo = uuid.lowBits.toString(16).padStart(16, '0');
  return `${hi.slice(0, 8)}-${hi.slice(8, 12)}-${hi.slice(12, 16)}-${lo.slice(0, 4)}-${lo.slice(4, 16)}`;
};

const stringToUuid = (input: string): Uuid => {
  const normalized = input.replace(/-/g, '').toLowerCase();
  if (!/^[0-9a-f]{32}$/.test(normalized)) {
    throw new ParamEncodingError(`invalid uuid: ${input}`);
  }
  return {
    highBits: BigInt('0x' + normalized.slice(0, 16)),
    lowBits: BigInt('0x' + normalized.slice(16, 32)),
  };
};

// ===========================================================================
// Tagged param machinery (per driver, but structurally identical)
// ===========================================================================

const PARAM_TAG = Symbol.for('golem-ts-sdk/fluent/rdbms/__param');

/** Tagged-value envelope produced by every `Pg`/`MySql`/`Ignite` helper. */
export interface DbParam<T extends string, V> {
  readonly [PARAM_TAG]: true;
  readonly driver: string;
  readonly kind: T;
  readonly value: V;
}

/** Internal factory for {@link DbParam} envelopes. */
export const makeParam =
  (driver: string) =>
  <T extends string, V>(kind: T, value: V): DbParam<T, V> => ({
    [PARAM_TAG]: true,
    driver,
    kind,
    value,
  });

const isDbParam = (driver: string, v: unknown): v is DbParam<string, unknown> =>
  typeof v === 'object' &&
  v !== null &&
  (v as Record<symbol, unknown>)[PARAM_TAG] === true &&
  (v as DbParam<string, unknown>).driver === driver;

// ===========================================================================
// POSTGRES codec
// ===========================================================================
//
// Faithful port of effect-golem `src/Postgres/internal/codec.ts`. Covers all
// `DbValue` variants: scalars, text/json, bytea, uuid, temporals, ranges,
// arrays, composite, domain, enumeration, vectors, inet/cidr/macaddr, bit.

// Minimal structural mirror of the postgres DbValue union — only what the
// codec constructs / reads. `unknown`-tagged escape hatches keep it from
// drifting out of sync with the (byte-compatible) host union.
type PgDbValue = { tag: string; val?: unknown };

export const pgParam = makeParam('postgres');

const pgRangeBound = (
  bound: { tag: 'included' | 'excluded' | 'unbounded'; val?: unknown },
  cast: (v: unknown) => unknown,
): { tag: string; val?: unknown } =>
  bound.tag === 'unbounded' ? { tag: 'unbounded' } : { tag: bound.tag, val: cast(bound.val) };

/** Encode a single JS / `Pg.*` parameter to a postgres `DbValue`. */
export const pgEncodeDbValue = (value: unknown): PgDbValue => {
  if (value === null || value === undefined) return { tag: 'null' };
  if (isDbParam('postgres', value)) return pgEncodeParam(value);
  switch (typeof value) {
    case 'string':
      return { tag: 'text', val: value };
    case 'boolean':
      return { tag: 'boolean', val: value };
    case 'bigint':
      checkInt64(value);
      return { tag: 'int8', val: value };
    case 'number': {
      if (Number.isNaN(value) || !Number.isFinite(value)) {
        throw new ParamEncodingError(
          'NaN / Infinity cannot be sent to postgres; use Pg.numeric(string)',
        );
      }
      if (isSafeInt32(value)) return { tag: 'int4', val: value };
      if (Number.isInteger(value)) return { tag: 'int8', val: toBigIntChecked(value, 'integer') };
      return { tag: 'float8', val: value };
    }
  }
  if (value instanceof Uint8Array) return { tag: 'bytea', val: value };
  if (value instanceof Date) return { tag: 'timestamptz', val: dateToTimestamptz(value) };
  throw new ParamEncodingError(
    `unsupported parameter type ${Object.prototype.toString.call(value)} — use a Pg.<helper>(...) wrapper`,
  );
};

const pgEncodeParam = (param: DbParam<string, unknown>): PgDbValue => {
  const v = param.value;
  switch (param.kind) {
    case 'json':
      return { tag: 'json', val: JSON.stringify(v) };
    case 'jsonb':
      return { tag: 'jsonb', val: JSON.stringify(v) };
    case 'jsonpath':
      return { tag: 'jsonpath', val: v as string };
    case 'xml':
      return { tag: 'xml', val: v as string };
    case 'uuid':
      return { tag: 'uuid', val: typeof v === 'string' ? stringToUuid(v) : (v as Uuid) };
    case 'array':
      return {
        tag: 'array',
        val: (v as ReadonlyArray<unknown>).map((e) => makeLazy(pgEncodeDbValue(e))),
      };
    case 'range': {
      const { range, hint } = v as { range: PgRange<unknown>; hint: PgRangeElementHint };
      const tagFor: Record<PgRangeElementHint, string> = {
        int4: 'int4range',
        int8: 'int8range',
        num: 'numrange',
        ts: 'tsrange',
        tstz: 'tstzrange',
        date: 'daterange',
      };
      return {
        tag: tagFor[hint],
        val: {
          start: pgRangeBound(range.start, (x) => x),
          end: pgRangeBound(range.end, (x) => x),
        },
      };
    }
    case 'composite': {
      const { name, values } = v as { name: string; values: ReadonlyArray<unknown> };
      return {
        tag: 'composite',
        val: { name, values: values.map((e) => makeLazy(pgEncodeDbValue(e))) },
      };
    }
    case 'domain': {
      const { name, value } = v as { name: string; value: unknown };
      return { tag: 'domain', val: { name, value: makeLazy(pgEncodeDbValue(value)) } };
    }
    case 'enumeration': {
      const { name, value } = v as { name: string; value: string };
      return { tag: 'enumeration', val: { name, value } };
    }
    case 'vector':
      return { tag: 'vector', val: (v as ReadonlyArray<number>).slice() };
    case 'halfvec':
      return { tag: 'halfvec', val: (v as ReadonlyArray<number>).slice() };
    case 'sparsevec': {
      const sv = v as PgSparseVec;
      return {
        tag: 'sparsevec',
        val: { dim: sv.dim, indices: sv.indices.slice(), values: sv.values.slice() },
      };
    }
    case 'numeric':
      return { tag: 'numeric', val: v as string };
    case 'interval':
      return { tag: 'interval', val: v };
    case 'inet':
      return { tag: 'inet', val: v as IpAddress };
    case 'cidr':
      return { tag: 'cidr', val: v as IpAddress };
    case 'macaddr':
      return { tag: 'macaddr', val: encodeMacAddr(v) };
    case 'bit':
      return { tag: 'bit', val: (v as ReadonlyArray<boolean>).slice() };
    case 'varbit':
      return { tag: 'varbit', val: (v as ReadonlyArray<boolean>).slice() };
    case 'int2':
      return { tag: 'int2', val: v as number };
    case 'int4':
      return { tag: 'int4', val: v as number };
    case 'int8': {
      const b = toBigIntChecked(v as number | bigint, 'Pg.int8');
      checkInt64(b);
      return { tag: 'int8', val: b };
    }
    case 'float4':
      return { tag: 'float4', val: v as number };
    case 'float8':
      return { tag: 'float8', val: v as number };
    case 'text':
      return { tag: 'text', val: v as string };
    case 'varchar':
      return { tag: 'varchar', val: v as string };
    case 'bpchar':
      return { tag: 'bpchar', val: v as string };
    case 'character':
      return { tag: 'character', val: v as number };
    case 'oid':
      return { tag: 'oid', val: v as number };
    case 'money':
      return { tag: 'money', val: v as bigint };
    case 'bytea':
      return { tag: 'bytea', val: v as Uint8Array };
    case 'timestamp':
      return { tag: 'timestamp', val: v };
    case 'timestamptz':
      return { tag: 'timestamptz', val: v };
    default:
      throw new ParamEncodingError(`unknown Pg helper: ${param.kind}`);
  }
};

const encodeMacAddr = (input: unknown): MacAddress => {
  if (Array.isArray(input)) {
    return { octets: input.slice(0, 6) as MacAddress['octets'] };
  }
  return input as MacAddress;
};

// A `makeLazy` shim: the host's `LazyDbValue` is a resource constructor; the
// codec only needs `{ get(): DbValue }`. For encoding the driver re-wraps these
// through the real `LazyDbValue` constructor (see postgres.ts). In the
// host-free codec path we use a plain object so round-trip tests work.
let lazyCtor: (<T>(v: T) => Lazy<T>) | undefined;
const makeLazy = <T>(v: T): Lazy<T> =>
  lazyCtor ? lazyCtor(v) : { get: () => v };

/**
 * Install the host's `LazyDbValue` constructor so nested array/composite/domain
 * params are wrapped in the real resource. Called once by `postgres.ts`. When
 * unset (e.g. in node tests) a plain `{ get }` shim is used.
 */
export const setPgLazyCtor = (ctor: <T>(v: T) => Lazy<T>): void => {
  lazyCtor = ctor;
};

const pgDecodeBound = (
  bound: { tag: string; val?: Lazy<PgDbValue> },
  mode: TemporalDecodeMode,
): unknown =>
  bound.tag === 'unbounded'
    ? { tag: 'unbounded' }
    : { tag: bound.tag, val: pgDecodeDbValue(bound.val!.get(), mode) };

/** Decode a postgres `DbValue` to a JS value. */
export const pgDecodeDbValue = (value: PgDbValue, mode: TemporalDecodeMode): unknown => {
  const val = value.val;
  switch (value.tag) {
    case 'null':
      return null;
    case 'character':
    case 'int2':
    case 'int4':
    case 'float4':
    case 'float8':
    case 'oid':
    case 'int8':
    case 'money':
    case 'numeric':
    case 'boolean':
    case 'text':
    case 'varchar':
    case 'bpchar':
    case 'json':
    case 'jsonb':
    case 'jsonpath':
    case 'xml':
    case 'bytea':
      return val;
    case 'uuid':
      return uuidToString(val as Uuid);
    case 'timestamp':
      return mode === 'date' ? timestampToDate(val as Timestamp) : val;
    case 'timestamptz':
      return mode === 'date' ? timestamptzToDate(val as Timestamptz) : val;
    case 'date':
      return mode === 'date'
        ? dateOnlyToDate(val as { year: number; month: number; day: number })
        : val;
    case 'time':
    case 'timetz':
    case 'interval':
    case 'inet':
    case 'cidr':
    case 'macaddr':
    case 'bit':
    case 'varbit':
    case 'int4range':
    case 'int8range':
    case 'numrange':
    case 'tsrange':
    case 'tstzrange':
    case 'daterange':
    case 'enumeration':
      return val;
    case 'composite': {
      const c = val as { name: string; values: ReadonlyArray<Lazy<PgDbValue>> };
      return { name: c.name, values: c.values.map((lv) => pgDecodeDbValue(lv.get(), mode)) };
    }
    case 'domain': {
      const d = val as { name: string; value: Lazy<PgDbValue> };
      return { name: d.name, value: pgDecodeDbValue(d.value.get(), mode) };
    }
    case 'array':
      return (val as ReadonlyArray<Lazy<PgDbValue>>).map((lv) => pgDecodeDbValue(lv.get(), mode));
    case 'range': {
      const r = val as {
        name: string;
        value: {
          start: { tag: string; val?: Lazy<PgDbValue> };
          end: { tag: string; val?: Lazy<PgDbValue> };
        };
      };
      return {
        name: r.name,
        value: { start: pgDecodeBound(r.value.start, mode), end: pgDecodeBound(r.value.end, mode) },
      };
    }
    case 'vector':
    case 'halfvec':
    case 'sparsevec':
      return val;
    default:
      return val;
  }
};

// Postgres helper-only types (re-exported through postgres.ts).
export type PgBound<T> =
  | { readonly tag: 'included'; readonly val: T }
  | { readonly tag: 'excluded'; readonly val: T }
  | { readonly tag: 'unbounded' };
export interface PgRange<T> {
  readonly start: PgBound<T>;
  readonly end: PgBound<T>;
}
export interface PgSparseVec {
  readonly dim: number;
  readonly indices: ReadonlyArray<number>;
  readonly values: ReadonlyArray<number>;
}
export type PgRangeElementHint = 'int4' | 'int8' | 'num' | 'ts' | 'tstz' | 'date';

// ===========================================================================
// MYSQL codec
// ===========================================================================
// Faithful port of effect-golem `src/Mysql/internal/codec.ts`.

type MyDbValue = { tag: string; val?: unknown };

export const mysqlParam = makeParam('mysql');

/** Encode a single JS / `MySql.*` parameter to a mysql `DbValue`. */
export const mysqlEncodeDbValue = (value: unknown): MyDbValue => {
  if (value === null || value === undefined) return { tag: 'null' };
  if (isDbParam('mysql', value)) return mysqlEncodeParam(value);
  switch (typeof value) {
    case 'string':
      return { tag: 'varchar', val: value };
    case 'boolean':
      return { tag: 'boolean', val: value };
    case 'bigint':
      checkInt64(value);
      return { tag: 'bigint', val: value };
    case 'number': {
      if (Number.isNaN(value) || !Number.isFinite(value)) {
        throw new ParamEncodingError(
          'NaN / Infinity cannot be sent to MySQL; use MySql.decimal(string)',
        );
      }
      if (isSafeInt32(value)) return { tag: 'int', val: value };
      if (Number.isInteger(value)) return { tag: 'bigint', val: toBigIntChecked(value, 'integer') };
      return { tag: 'double', val: value };
    }
  }
  if (value instanceof Uint8Array) return { tag: 'blob', val: value };
  if (value instanceof Date) return { tag: 'datetime', val: dateToTimestamp(value) };
  throw new ParamEncodingError(
    `unsupported parameter type ${Object.prototype.toString.call(value)} — use a MySql.<helper>(...) wrapper`,
  );
};

const mysqlEncodeParam = (param: DbParam<string, unknown>): MyDbValue => {
  const v = param.value;
  switch (param.kind) {
    case 'json':
      return { tag: 'json', val: JSON.stringify(v) };
    case 'enumeration':
      return { tag: 'enumeration', val: v as string };
    case 'set':
      return { tag: 'set', val: v as string };
    case 'bit':
      return { tag: 'bit', val: (v as ReadonlyArray<boolean>).slice() };
    case 'decimal':
      return { tag: 'decimal', val: v as string };
    case 'year':
      return { tag: 'year', val: v as number };
    case 'tinyint':
    case 'smallint':
    case 'mediumint':
    case 'int':
    case 'tinyint-unsigned':
    case 'smallint-unsigned':
    case 'mediumint-unsigned':
    case 'int-unsigned':
      return { tag: param.kind, val: v as number };
    case 'bigint': {
      const b = toBigIntChecked(v as number | bigint, 'MySql.bigint');
      checkInt64(b);
      return { tag: 'bigint', val: b };
    }
    case 'bigint-unsigned': {
      const b = toBigIntChecked(v as number | bigint, 'MySql.bigintUnsigned');
      checkUint64(b, 'MySql.bigintUnsigned');
      return { tag: 'bigint-unsigned', val: b };
    }
    case 'float':
      return { tag: 'float', val: v as number };
    case 'double':
      return { tag: 'double', val: v as number };
    case 'fixchar':
    case 'varchar':
    case 'tinytext':
    case 'text':
    case 'mediumtext':
    case 'longtext':
      return { tag: param.kind, val: v as string };
    case 'binary':
    case 'varbinary':
    case 'tinyblob':
    case 'blob':
    case 'mediumblob':
    case 'longblob':
      return { tag: param.kind, val: v as Uint8Array };
    case 'date':
      return { tag: 'date', val: v };
    case 'time':
      return { tag: 'time', val: v };
    case 'datetime':
      return { tag: 'datetime', val: v };
    case 'timestamp':
      return { tag: 'timestamp', val: v };
    default:
      throw new ParamEncodingError(`unknown MySql helper: ${param.kind}`);
  }
};

/** Decode a mysql `DbValue` to a JS value. */
export const mysqlDecodeDbValue = (value: MyDbValue, mode: TemporalDecodeMode): unknown => {
  const val = value.val;
  switch (value.tag) {
    case 'null':
      return null;
    case 'datetime':
    case 'timestamp':
      return mode === 'date' ? timestampToDate(val as Timestamp) : val;
    case 'date':
      return mode === 'date'
        ? dateOnlyToDate(val as { year: number; month: number; day: number })
        : val;
    default:
      // boolean, all int kinds, year, float, double, decimal, all text kinds,
      // json, enumeration, set, all blob/binary kinds, bit, time.
      return val;
  }
};

// ===========================================================================
// IGNITE codec
// ===========================================================================
// Faithful port of effect-golem `src/Ignite/internal/codec.ts`.

type IgDbValue = { tag: string; val?: unknown };

export const igniteParam = makeParam('ignite');

/** Ignite uuid — 36-char string, `{hi,lo}` struct, or `[hi,lo]` tuple. */
export type IgniteUuid = string | { readonly hi: bigint; readonly lo: bigint } | [bigint, bigint];

const igniteEncodeUuid = (input: IgniteUuid): [bigint, bigint] => {
  if (typeof input === 'string') {
    const normalized = input.replace(/-/g, '').toLowerCase();
    if (!/^[0-9a-f]{32}$/.test(normalized)) {
      throw new ParamEncodingError(`invalid uuid: ${input}`);
    }
    return [BigInt('0x' + normalized.slice(0, 16)), BigInt('0x' + normalized.slice(16, 32))];
  }
  const hi = Array.isArray(input) ? input[0] : input.hi;
  const lo = Array.isArray(input) ? input[1] : input.lo;
  if (typeof hi !== 'bigint' || typeof lo !== 'bigint') {
    throw new ParamEncodingError('Ignite.uuid hi/lo must both be bigint');
  }
  checkUint64(hi, 'Ignite.uuid hi');
  checkUint64(lo, 'Ignite.uuid lo');
  return [hi, lo];
};

const dateToEpochMillis = (d: Date): bigint => {
  const ms = d.getTime();
  if (!Number.isFinite(ms)) throw new ParamEncodingError('Date is not a valid timestamp');
  return BigInt(ms);
};

const igniteUuidToString = (u: [bigint, bigint]): string => {
  const hi = u[0].toString(16).padStart(16, '0');
  const lo = u[1].toString(16).padStart(16, '0');
  return `${hi.slice(0, 8)}-${hi.slice(8, 12)}-${hi.slice(12, 16)}-${lo.slice(0, 4)}-${lo.slice(4, 16)}`;
};

/** Encode a single JS / `Ignite.*` parameter to an ignite `DbValue`. */
export const igniteEncodeDbValue = (value: unknown): IgDbValue => {
  if (value === null || value === undefined) return { tag: 'db-null' };
  if (isDbParam('ignite', value)) return igniteEncodeParam(value);
  switch (typeof value) {
    case 'string':
      return { tag: 'db-string', val: value };
    case 'boolean':
      return { tag: 'db-boolean', val: value };
    case 'bigint':
      checkInt64(value);
      return { tag: 'db-long', val: value };
    case 'number': {
      if (Number.isNaN(value) || !Number.isFinite(value)) {
        throw new ParamEncodingError(
          'NaN / Infinity cannot be sent to Ignite; use Ignite.decimal(string)',
        );
      }
      if (isSafeInt32(value)) return { tag: 'db-int', val: value };
      if (Number.isInteger(value)) {
        return { tag: 'db-long', val: toBigIntChecked(value, 'integer') };
      }
      return { tag: 'db-double', val: value };
    }
  }
  if (value instanceof Uint8Array) return { tag: 'db-byte-array', val: value };
  if (value instanceof Date) return { tag: 'db-date', val: dateToEpochMillis(value) };
  throw new ParamEncodingError(
    `unsupported parameter type ${Object.prototype.toString.call(value)} — use an Ignite.<helper>(...) wrapper`,
  );
};

const igniteEncodeParam = (param: DbParam<string, unknown>): IgDbValue => {
  const v = param.value;
  switch (param.kind) {
    case 'uuid':
      return { tag: 'db-uuid', val: igniteEncodeUuid(v as IgniteUuid) };
    case 'decimal':
      return { tag: 'db-decimal', val: v as string };
    case 'date': {
      const x = v as bigint | number | Date;
      return {
        tag: 'db-date',
        val: x instanceof Date ? dateToEpochMillis(x) : toBigIntChecked(x, 'Ignite.date millis'),
      };
    }
    case 'timestamp': {
      const { millis, subMilliNanos } = v as { millis: bigint | number; subMilliNanos: number };
      const m = toBigIntChecked(millis, 'Ignite.timestamp millis');
      if (!Number.isInteger(subMilliNanos) || subMilliNanos < 0 || subMilliNanos > 999_999) {
        throw new ParamEncodingError(
          `Ignite.timestamp sub-ms-nanos must be in 0..999_999; got ${String(subMilliNanos)}`,
        );
      }
      return { tag: 'db-timestamp', val: [m, subMilliNanos] };
    }
    case 'time':
      return { tag: 'db-time', val: toBigIntChecked(v as bigint | number, 'Ignite.time nanos') };
    case 'char':
      return { tag: 'db-char', val: v as number };
    case 'byte-array':
      return { tag: 'db-byte-array', val: v as Uint8Array };
    case 'byte':
      return { tag: 'db-byte', val: v as number };
    case 'short':
      return { tag: 'db-short', val: v as number };
    case 'int':
      return { tag: 'db-int', val: v as number };
    case 'long': {
      const b = toBigIntChecked(v as number | bigint, 'Ignite.long');
      checkInt64(b);
      return { tag: 'db-long', val: b };
    }
    case 'float':
      return { tag: 'db-float', val: v as number };
    case 'double':
      return { tag: 'db-double', val: v as number };
    case 'string':
      return { tag: 'db-string', val: v as string };
    case 'boolean':
      return { tag: 'db-boolean', val: v as boolean };
    default:
      throw new ParamEncodingError(`unknown Ignite helper: ${param.kind}`);
  }
};

/** Decode an ignite `DbValue` to a JS value. */
export const igniteDecodeDbValue = (value: IgDbValue, mode: TemporalDecodeMode): unknown => {
  const val = value.val;
  switch (value.tag) {
    case 'db-null':
      return null;
    case 'db-uuid':
      return igniteUuidToString(val as [bigint, bigint]);
    case 'db-date':
      return mode === 'date' ? new Date(Number(val as bigint)) : val;
    case 'db-timestamp':
      return mode === 'date' ? new Date(Number((val as [bigint, number])[0])) : val;
    default:
      // db-boolean, db-byte/short/int/float/double/char, db-long, db-string,
      // db-decimal, db-time, db-byte-array.
      return val;
  }
};

// ===========================================================================
// Shared row decoding
// ===========================================================================

/** Decode rows into an array of `{ columnName: value }` records. */
export const decodeRows = <V>(
  rows: ReadonlyArray<DbRowLike<V>>,
  columns: ReadonlyArray<DbColumnLike>,
  decodeValue: (v: V, mode: TemporalDecodeMode) => unknown,
  mode: TemporalDecodeMode,
): Array<Record<string, unknown>> => {
  const colNames = columns.map((c) => c.name);
  return rows.map((row) => {
    const obj: Record<string, unknown> = {};
    for (let i = 0; i < colNames.length; i++) {
      obj[colNames[i]!] = decodeValue(row.values[i]!, mode);
    }
    return obj;
  });
};

/** Decode rows into positional value arrays (no column names). */
export const decodeRowsValues = <V>(
  rows: ReadonlyArray<DbRowLike<V>>,
  decodeValue: (v: V, mode: TemporalDecodeMode) => unknown,
  mode: TemporalDecodeMode,
): Array<Array<unknown>> => rows.map((row) => row.values.map((v) => decodeValue(v, mode)));
