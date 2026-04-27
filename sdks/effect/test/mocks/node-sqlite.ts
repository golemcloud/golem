/**
 * In-memory mock for the `node:sqlite` module + Golem's wasm-rquickjs
 * extensions (`serializeDatabaseSync`, `restoreDatabaseSync`,
 * `isAutocommitDatabaseSync`).
 *
 * The mock is intentionally minimal:
 *
 * - `DatabaseSync` is a marker class with a tiny query mock so a few
 *   `prepare(...)`-like calls don't blow up. Real query semantics live
 *   in unit tests that opt in to using the actual `node:sqlite` import.
 * - The three extension functions delegate to module-level overrides
 *   that tests can swap out via `__setSerializeDatabaseSync` etc.
 *   This mirrors the SDK's own `__setGetEnvironmentForTest` pattern.
 */

export type RowMode = "object" | "array"

export class StatementSync {
  constructor(
    _db: DatabaseSync,
    private readonly _sql: string,
  ) {}
  setReadBigInts(_v: boolean): this {
    return this
  }
  setAllowBareNamedParameters(_v: boolean): this {
    return this
  }
  setReturnArrays(_v: boolean): this {
    return this
  }
  all(..._params: ReadonlyArray<unknown>): Array<Record<string, unknown>> {
    return []
  }
  get(..._params: ReadonlyArray<unknown>): Record<string, unknown> | undefined {
    return undefined
  }
  run(..._params: ReadonlyArray<unknown>): { changes: number; lastInsertRowid: number | bigint } {
    return { changes: 0, lastInsertRowid: 0 }
  }
  iterate(..._params: ReadonlyArray<unknown>): IterableIterator<Record<string, unknown>> {
    return [][Symbol.iterator]()
  }
  finalize(): void {
    /* noop */
  }
  get sourceSQL(): string {
    return this._sql
  }
}

export class Session {
  changeset(): Uint8Array {
    return new Uint8Array(0)
  }
  patchset(): Uint8Array {
    return new Uint8Array(0)
  }
  close(): void {
    /* noop */
  }
  [Symbol.dispose](): void {
    /* noop */
  }
}

export class SQLTagStore {
  readonly capacity: number
  readonly db: DatabaseSync
  size: number = 0
  constructor(db: DatabaseSync, maxSize?: number) {
    this.db = db
    this.capacity = maxSize ?? 0
  }
  get(_strings: TemplateStringsArray, ..._values: ReadonlyArray<unknown>): unknown {
    return undefined
  }
  all(_strings: TemplateStringsArray, ..._values: ReadonlyArray<unknown>): Array<unknown> {
    return []
  }
  run(_strings: TemplateStringsArray, ..._values: ReadonlyArray<unknown>): unknown {
    return undefined
  }
  iterate(
    _strings: TemplateStringsArray,
    ..._values: ReadonlyArray<unknown>
  ): IterableIterator<unknown> {
    return [][Symbol.iterator]()
  }
  clear(): void {
    /* noop */
  }
}

export class DatabaseSync {
  /** Test-only metadata so mocks can verify which DB is which. */
  public readonly __id: number
  private static _counter = 0
  public _closed = false
  public _execLog: Array<string> = []
  public _bytes: Uint8Array = new Uint8Array(0)
  public _autocommit = true
  public _attachments: Array<{ name: string }> = [{ name: "main" }, { name: "temp" }]

  constructor(_path: unknown, _options?: unknown) {
    this.__id = ++DatabaseSync._counter
  }

  open(): void {
    this._closed = false
  }
  close(): void {
    this._closed = true
  }
  exec(sql: string): void {
    this._execLog.push(sql)
  }
  prepare(sql: string): StatementSync {
    // Special-case the autocommit/list pragmas for snapshot tests.
    const stmt = new StatementSync(this, sql)
    const lower = sql.trim().toLowerCase()
    if (lower === "pragma database_list") {
      stmt.all = (..._args: ReadonlyArray<unknown>) =>
        this._attachments.map((a) => ({ name: a.name })) as Array<Record<string, unknown>>
    }
    return stmt
  }
  function(_name: string, _fn: (...args: ReadonlyArray<unknown>) => unknown): void {
    /* noop */
  }
  aggregate(_name: string, _options: unknown): void {
    /* noop */
  }
  createSession(_options?: unknown): Session {
    return new Session()
  }
  applyChangeset(_changeset: Uint8Array, _options?: unknown): boolean {
    return true
  }
  enableLoadExtension(_v: boolean): void {
    /* noop */
  }
  loadExtension(_path: string): void {
    /* noop */
  }
  backup(_path: unknown, _options?: unknown): Promise<void> {
    return Promise.resolve()
  }
  isOpen(): boolean {
    return !this._closed
  }
  isTransaction(): boolean {
    return !this._autocommit
  }
  [Symbol.dispose](): void {
    this.close()
  }
}

// ---------------------------------------------------------------------------
// Wasm-rquickjs extensions — module-level swappable.
// ---------------------------------------------------------------------------

let serializeImpl: (db: DatabaseSync) => Uint8Array = (db) => {
  // Default: return whatever bytes the test wrote into _bytes (or a
  // synthetic marker so the test can recognise it).
  if (db._bytes.length > 0) return db._bytes
  return new TextEncoder().encode(`db#${db.__id}`)
}

let restoreImpl: (db: DatabaseSync, bytes: Uint8Array) => void = (db, bytes) => {
  db._bytes = bytes
}

let isAutocommitImpl: (db: DatabaseSync) => boolean = (db) => db._autocommit

export const serializeDatabaseSync = (db: DatabaseSync): Uint8Array => serializeImpl(db)
export const restoreDatabaseSync = (db: DatabaseSync, bytes: Uint8Array): void =>
  restoreImpl(db, bytes)
export const isAutocommitDatabaseSync = (db: DatabaseSync): boolean => isAutocommitImpl(db)

export const __setSerializeDatabaseSync = (fn: (db: DatabaseSync) => Uint8Array): void => {
  serializeImpl = fn
}
export const __setRestoreDatabaseSync = (
  fn: (db: DatabaseSync, bytes: Uint8Array) => void,
): void => {
  restoreImpl = fn
}
export const __setIsAutocommitDatabaseSync = (fn: (db: DatabaseSync) => boolean): void => {
  isAutocommitImpl = fn
}
export const __resetSqliteExtensions = (): void => {
  serializeImpl = (db) => {
    if (db._bytes.length > 0) return db._bytes
    return new TextEncoder().encode(`db#${db.__id}`)
  }
  restoreImpl = (db, bytes) => {
    db._bytes = bytes
  }
  isAutocommitImpl = (db) => db._autocommit
}

export const backup = (
  _sourceDb: DatabaseSync,
  _path: unknown,
  _options?: unknown,
): Promise<void> => Promise.resolve()

export const constants = Object.freeze({})
