import { Duration, Effect, Ref, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type { DatabaseSync } from "node:sqlite"
import { Principal } from "./principal.js"
import { __getUnderlyingDatabase, isSqliteClient, type SqliteClient } from "./sqlite.js"
import { toWitCodec, UnsupportedSchemaError, type WitCodec } from "./wit-codec.js"

/**
 * Per-agent snapshotting configuration.
 *
 * Snapshotting is opt-in: an agent without a `snapshot` field maps to
 * `agent-type.snapshotting = disabled` and the runtime never invokes
 * `save`/`load`. When opted in via {@link Snapshot.define} (auto,
 * schema-driven) or {@link Snapshot.custom} (user-managed bytes), the
 * SDK:
 *
 * - emits the matching `snapshotting` value into the agent type
 *   metadata,
 * - constructs a fresh {@link SnapshotBinding} per agent instance and
 *   passes it to `impl` as the second argument,
 * - exposes save/load through the host's
 *   `golem:api/save-snapshot@1.5.0` / `golem:api/load-snapshot@1.5.0`
 *   exports, with envelopes that are bit-for-bit compatible with the
 *   official `golem-ts-sdk`.
 *
 * On restore the host calls `load-snapshot.load` instead of
 * `agent-guest.guest.initialize`. The SDK reads the agent's own ID
 * (via `wasi:cli/environment` / `golem:agent/host.parse-agent-id`),
 * runs the constructor with those parameters, then applies the
 * snapshot.
 */

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/**
 * Snapshotting policy mirroring the WIT
 * `golem:agent/common.snapshotting-config` variant. `manual` is an
 * alias for `default` (the official TS SDK names it that way).
 */
export type SnapshotPolicy =
  | { readonly _tag: "Default" }
  | { readonly _tag: "Periodic"; readonly duration: Duration.Input }
  | { readonly _tag: "EveryN"; readonly n: number }

const policyDefault: SnapshotPolicy = { _tag: "Default" }
const policyPeriodic = (duration: Duration.Input): SnapshotPolicy => ({
  _tag: "Periodic",
  duration,
})
const policyEveryN = (n: number): SnapshotPolicy => ({ _tag: "EveryN", n })

/** Namespace of policy constructors used inside `Snapshot.define` / `Snapshot.custom`. */
export const policy = {
  /** Host-default snapshotting cadence. */
  default: policyDefault,
  /** Alias for {@link policy.default}; matches `golem-ts-sdk` naming. */
  manual: policyDefault,
  /** Snapshot on a fixed time interval. */
  periodic: policyPeriodic,
  /** Snapshot every `n` invocations. `n` is a `u16` in WIT (1..=65535). */
  everyN: policyEveryN,
} as const

const isFiniteInteger = (n: number): boolean => Number.isInteger(n) && Number.isFinite(n)

const policyToWit = (
  p: SnapshotPolicy,
  ctxLabel: string,
): Effect.Effect<AgentCommon.SnapshottingConfig, InvalidSnapshotError> => {
  switch (p._tag) {
    case "Default":
      return Effect.succeed({ tag: "default" })
    case "Periodic": {
      const d = Duration.fromInputUnsafe(p.duration)
      const nanos = Duration.toNanosUnsafe(d)
      // wasi:clocks/monotonic-clock.duration = u64 nanoseconds.
      return Effect.succeed({ tag: "periodic", val: nanos < 0n ? 0n : nanos })
    }
    case "EveryN": {
      if (!isFiniteInteger(p.n) || p.n < 1 || p.n > 0xffff) {
        return Effect.fail(
          new InvalidSnapshotError(
            `${ctxLabel}: snapshot policy 'everyN' must be a positive integer in 1..=65535 (got ${p.n})`,
          ),
        )
      }
      return Effect.succeed({ tag: "every-n-invocation", val: p.n })
    }
  }
}

// ---------------------------------------------------------------------------
// Typed errors
// ---------------------------------------------------------------------------

/** Surfaced from `registerAgent` for malformed `Snapshot.define`/`Snapshot.custom` shapes. */
export class InvalidSnapshotError {
  readonly _tag = "InvalidSnapshotError"
  readonly message: string
  constructor(readonly reason: string) {
    this.message = `InvalidSnapshotError: ${reason}`
  }
}

/** Raised when an agent declared `snapshot` but `impl` never called `init`/`register`. */
export class SnapshotNotBoundError {
  readonly _tag = "SnapshotNotBoundError"
  readonly message: string
  constructor(readonly agentName: string) {
    this.message = `SnapshotNotBoundError: agent '${agentName}' declared a snapshot but did not call snap.init / snap.register inside impl`
  }
}

/** Raised when `init`/`register` is called more than once during a single agent lifetime. */
export class SnapshotAlreadyBoundError {
  readonly _tag = "SnapshotAlreadyBoundError"
  readonly message: string
  constructor(readonly agentName: string) {
    this.message = `SnapshotAlreadyBoundError: agent '${agentName}' has already bound its snapshot — snap.init / snap.register may only be called once per impl`
  }
}

/** Raised when `attachDatabase` is called more than once for the same name. */
export class SnapshotDatabaseDuplicateAttachError {
  readonly _tag = "SnapshotDatabaseDuplicateAttachError"
  readonly message: string
  constructor(
    readonly agentName: string,
    readonly databaseName: string,
  ) {
    this.message = `SnapshotDatabaseDuplicateAttachError: agent '${agentName}' attached database '${databaseName}' more than once`
  }
}

/**
 * Raised when, at save or load time, a declared database name has no
 * corresponding `attachDatabase` call (save) or no corresponding part
 * in the loaded envelope (load).
 */
export class SnapshotDatabaseMissingPartError {
  readonly _tag = "SnapshotDatabaseMissingPartError"
  readonly message: string
  constructor(
    readonly agentName: string,
    readonly databaseName: string,
    readonly phase: "save" | "load",
  ) {
    this.message =
      phase === "save"
        ? `SnapshotDatabaseMissingPartError: agent '${agentName}' declared database '${databaseName}' but never called snap.attachDatabase('${databaseName}', ...) inside impl`
        : `SnapshotDatabaseMissingPartError: agent '${agentName}' load: snapshot envelope is missing required 'db:${databaseName}' part`
  }
}

/**
 * Raised at load time when a snapshot envelope contains a `db:<name>`
 * part whose `<name>` is not in the agent's declared `databases`
 * tuple.
 */
export class SnapshotDatabaseUnknownPartError {
  readonly _tag = "SnapshotDatabaseUnknownPartError"
  readonly message: string
  constructor(
    readonly agentName: string,
    readonly databaseName: string,
  ) {
    this.message = `SnapshotDatabaseUnknownPartError: agent '${agentName}' load: snapshot envelope carries 'db:${databaseName}' but the agent did not declare a database with that name`
  }
}

/**
 * Raised at save time when a declared database has an open transaction
 * (`isAutocommitDatabaseSync` returns false).
 */
export class SnapshotDatabaseNotInAutocommitError {
  readonly _tag = "SnapshotDatabaseNotInAutocommitError"
  readonly message: string
  constructor(
    readonly agentName: string,
    readonly databaseName: string,
  ) {
    this.message = `SnapshotDatabaseNotInAutocommitError: agent '${agentName}' database '${databaseName}' has an open transaction; commit or rollback before snapshot save`
  }
}

/**
 * Raised at save time when a declared database has ATTACHed schemas
 * beyond the default `main`/`temp` (PRAGMA database_list).
 */
export class SnapshotDatabaseHasAttachmentsError {
  readonly _tag = "SnapshotDatabaseHasAttachmentsError"
  readonly message: string
  constructor(
    readonly agentName: string,
    readonly databaseName: string,
    readonly extraSchemas: ReadonlyArray<string>,
  ) {
    this.message = `SnapshotDatabaseHasAttachmentsError: agent '${agentName}' database '${databaseName}' has ATTACHed schemas (${extraSchemas.join(", ")}) — only main/temp may be present at snapshot time`
  }
}

// ---------------------------------------------------------------------------
// Definition shapes
// ---------------------------------------------------------------------------

declare const snapshotDefBrand: unique symbol

/** Pattern that database names must match. */
const DB_NAME_RE = /^[a-zA-Z_][a-zA-Z0-9_]*$/

/**
 * Schema-driven snapshot definition. Produced by {@link Snapshot.define}.
 *
 * `State` is the in-memory shape the user wants persisted. The SDK
 * manages a `Ref.Ref<State>` and (when the host calls `save`) encodes
 * its current value via the provided `Schema.Top`, JSON-stringifies it,
 * and wraps the result in the JSON envelope.
 *
 * The optional `databases` tuple declares one or more SQLite databases
 * that should be captured alongside the auto state. Each name must be
 * registered exactly once via the per-instance binding's
 * `attachDatabase(name, db)` call. When `databases` is non-empty the
 * envelope on the wire becomes `multipart/mixed` with one
 * `application/x-sqlite3` part per declared database.
 */
export interface AutoSnapshotDef<S extends Schema.Top, DBs extends ReadonlyArray<string> = []> {
  readonly _tag: "AutoSnapshotDef"
  readonly schema: S
  readonly policy: SnapshotPolicy
  readonly databases?: DBs
  /** Phantom marker so `SnapshotBinding<S>` can recover `State`. */
  readonly [snapshotDefBrand]?: S["Type"]
}

/**
 * User-managed snapshot definition. Produced by {@link Snapshot.custom}.
 *
 * The user is responsible for serializing/deserializing their own state
 * through the `register({ save, load })` call inside `impl`.
 */
export interface CustomSnapshotDef {
  readonly _tag: "CustomSnapshotDef"
  readonly policy: SnapshotPolicy
}

/** Either flavour of snapshot definition that may appear on `AgentDefinition.snapshot`. */
export type SnapshotDef = AutoSnapshotDef<Schema.Top, ReadonlyArray<string>> | CustomSnapshotDef

/**
 * The per-instance binding object the dispatcher passes to `impl` as
 * its second argument when an agent declares a `snapshot` field.
 *
 * The shape depends on which definition variant was used: `init` +
 * `attachDatabase` for auto, `register` for custom.
 */
export type SnapshotBinding<S> =
  S extends AutoSnapshotDef<infer Sc, infer DBs>
    ? AutoSnapshotBinding<Sc, DBs>
    : S extends CustomSnapshotDef
      ? CustomSnapshotBinding
      : never

/** Acceptable second argument to `attachDatabase`. */
export type AttachableDatabase = SqliteClient | DatabaseSync

/** Auto-variant binding: yields a `Ref` initialised by the user. */
export interface AutoSnapshotBinding<S extends Schema.Top, DBs extends ReadonlyArray<string> = []> {
  /**
   * Allocate the snapshotted `Ref.Ref<State>` with the supplied
   * `initial` value and register it with the SDK as the snapshot
   * source. Must be called exactly once inside `impl`.
   */
  readonly init: (
    initial: S["Type"],
  ) => Effect.Effect<Ref.Ref<S["Type"]>, SnapshotAlreadyBoundError>
  /**
   * Register a SQLite database to be captured alongside the auto
   * state. `name` must be one of the names declared on
   * `Snapshot.define({ databases: ... })`. Each declared name must be
   * attached exactly once before `impl` returns, otherwise
   * `dispatchSaveSnapshot` raises {@link SnapshotDatabaseMissingPartError}.
   */
  readonly attachDatabase: (
    name: DBs[number],
    db: AttachableDatabase,
  ) => Effect.Effect<void, SnapshotDatabaseDuplicateAttachError>
}

/** Custom-variant binding: lets the user provide save/load Effects. */
export interface CustomSnapshotBinding {
  /**
   * Provide the per-instance `save` and `load` effects. Must be called
   * exactly once inside `impl`. `save` returns the raw user payload;
   * the SDK wraps it in the binary v2 envelope. `load` is invoked with
   * the inner user payload, post-envelope-decoding.
   */
  readonly register: (
    handlers: CustomSnapshotHandlers,
  ) => Effect.Effect<void, SnapshotAlreadyBoundError>
}

export interface CustomSnapshotHandlers {
  readonly save: Effect.Effect<Uint8Array, unknown, Principal>
  readonly load: (payload: Uint8Array) => Effect.Effect<void, unknown, Principal>
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/**
 * Schema-driven snapshot definition (the typical case): the SDK manages
 * a `Ref.Ref<State>` whose snapshot encoding is driven by the supplied
 * `Schema.Top`.
 *
 * The returned value goes into `defineAgent({ snapshot: ... })`. Inside
 * `impl`, the second argument is a {@link AutoSnapshotBinding} whose
 * `init(initial)` produces the actual `Ref`.
 *
 * The optional `databases` tuple — typically declared as
 * `databases: ["counters"] as const` — pre-declares one or more
 * SQLite databases that must each be attached exactly once via
 * `snap.attachDatabase(name, db)` before `impl` returns. The set of
 * accepted names is reflected at compile time in the binding's
 * `attachDatabase` first argument.
 */
export const define = <S extends Schema.Top, const DBs extends ReadonlyArray<string> = []>(spec: {
  readonly schema: S
  readonly policy: SnapshotPolicy
  readonly databases?: DBs
}): AutoSnapshotDef<S, DBs> =>
  spec.databases !== undefined
    ? {
        _tag: "AutoSnapshotDef",
        schema: spec.schema,
        policy: spec.policy,
        databases: spec.databases,
      }
    : {
        _tag: "AutoSnapshotDef",
        schema: spec.schema,
        policy: spec.policy,
      }

/**
 * User-managed snapshot definition: the user provides per-instance
 * `save`/`load` effects from inside `impl` via the
 * {@link CustomSnapshotBinding} that the dispatcher passes in.
 */
export const custom = (spec: { readonly policy: SnapshotPolicy }): CustomSnapshotDef => ({
  _tag: "CustomSnapshotDef",
  policy: spec.policy,
})

// ---------------------------------------------------------------------------
// Compiled bundle (consumed by agent.ts)
// ---------------------------------------------------------------------------

/** Compiled view of the user's snapshot definition. */
export type CompiledSnapshot =
  | {
      readonly kind: "auto"
      readonly policy: SnapshotPolicy
      readonly witConfig: AgentCommon.SnapshottingConfig
      readonly schema: Schema.Top
      /** Pre-built WIT codec (for parity / future use); not strictly required for JSON. */
      readonly witCodec: WitCodec<Schema.Top>
      /**
       * The database names declared by the user (deduplicated), in the
       * order they were given. Empty when no `databases` field was
       * supplied — in that case the agent stays on the plain JSON
       * envelope path.
       */
      readonly declaredDatabases: ReadonlyArray<string>
    }
  | {
      readonly kind: "custom"
      readonly policy: SnapshotPolicy
      readonly witConfig: AgentCommon.SnapshottingConfig
    }

/**
 * Compile a {@link SnapshotDef} produced by `Snapshot.define` /
 * `Snapshot.custom`. Walks the schema (auto path), validates the
 * policy, and produces the WIT-side `snapshotting-config`.
 */
export const compileSnapshot = (
  agentName: string,
  def: SnapshotDef,
): Effect.Effect<CompiledSnapshot, UnsupportedSchemaError | InvalidSnapshotError> =>
  Effect.gen(function* () {
    const witConfig = yield* policyToWit(def.policy, `agent '${agentName}' snapshot`)
    if (def._tag === "AutoSnapshotDef") {
      const witCodec = (yield* toWitCodec(def.schema)) as WitCodec<Schema.Top>
      const rawDbs = def.databases ?? []
      const seenNames = new Set<string>()
      for (const name of rawDbs) {
        if (!DB_NAME_RE.test(name)) {
          return yield* Effect.fail(
            new InvalidSnapshotError(
              `agent '${agentName}' snapshot: database name '${name}' is invalid; expected /^[a-zA-Z_][a-zA-Z0-9_]*$/`,
            ),
          )
        }
        if (seenNames.has(name)) {
          return yield* Effect.fail(
            new InvalidSnapshotError(
              `agent '${agentName}' snapshot: database name '${name}' appears more than once in 'databases'`,
            ),
          )
        }
        seenNames.add(name)
      }
      return {
        kind: "auto",
        policy: def.policy,
        witConfig,
        schema: def.schema,
        witCodec,
        declaredDatabases: [...rawDbs],
      }
    }
    return { kind: "custom", policy: def.policy, witConfig }
  })

// ---------------------------------------------------------------------------
// Per-instance binding + bound state
// ---------------------------------------------------------------------------

/**
 * Result of a successful `impl` for an agent that declared a snapshot.
 * Read by the dispatcher after `impl` returns; carries the actual Ref
 * (auto) or save/load effects (custom) the user bound from inside
 * `impl`.
 */
export type BoundSnapshot =
  | {
      readonly kind: "auto"
      readonly ref: Ref.Ref<unknown>
      readonly schema: Schema.Top
      /** Names declared on `Snapshot.define({ databases })`. */
      readonly declaredDatabases: ReadonlyArray<string>
      /** DBs the user attached via `attachDatabase(name, db)`. */
      readonly databases: ReadonlyMap<string, DatabaseSync>
    }
  | {
      readonly kind: "custom"
      readonly handlers: CustomSnapshotHandlers
    }

/**
 * Dispatcher-internal: a binding object plus a way to read whatever the
 * user bound. The same shape underlies both `init` (auto) and
 * `register` (custom).
 */
export interface BindingHandle {
  readonly binding: SnapshotBinding<SnapshotDef>
  readonly read: () => BoundSnapshot | null
}

/**
 * Construct a fresh per-instance binding for the given compiled
 * snapshot definition. The dispatcher passes the resulting `binding` to
 * `impl` as its second argument; after `impl` resolves, the dispatcher
 * calls `read()` to capture whatever the user bound (or `null` if
 * nothing was bound — that's a `SnapshotNotBoundError`).
 */
export const createBinding = (agentName: string, compiled: CompiledSnapshot): BindingHandle => {
  let bound: BoundSnapshot | null = null
  if (compiled.kind === "auto") {
    const declaredSet = new Set(compiled.declaredDatabases)
    const databases = new Map<string, DatabaseSync>()
    const auto: AutoSnapshotBinding<Schema.Top, ReadonlyArray<string>> = {
      init: (initial) =>
        Effect.gen(function* () {
          if (bound !== null) {
            return yield* Effect.fail(new SnapshotAlreadyBoundError(agentName))
          }
          const ref = yield* Ref.make(initial as unknown)
          bound = {
            kind: "auto",
            ref,
            schema: compiled.schema,
            declaredDatabases: compiled.declaredDatabases,
            databases,
          }
          return ref
        }),
      attachDatabase: (name, db) =>
        Effect.suspend(() => {
          if (!declaredSet.has(name)) {
            return Effect.fail(
              new SnapshotDatabaseDuplicateAttachError(
                agentName,
                `${name}: not declared in 'databases'`,
              ),
            )
          }
          if (databases.has(name)) {
            return Effect.fail(new SnapshotDatabaseDuplicateAttachError(agentName, name))
          }
          const handle = isSqliteClient(db) ? __getUnderlyingDatabase(db) : (db as DatabaseSync)
          databases.set(name, handle)
          return Effect.void
        }),
    }
    return {
      binding: auto as unknown as SnapshotBinding<SnapshotDef>,
      read: () => bound,
    }
  }
  const customBinding: CustomSnapshotBinding = {
    register: (handlers) =>
      Effect.suspend(() => {
        if (bound !== null) {
          return Effect.fail(new SnapshotAlreadyBoundError(agentName))
        }
        bound = { kind: "custom", handlers }
        return Effect.void
      }),
  }
  return {
    binding: customBinding as unknown as SnapshotBinding<SnapshotDef>,
    read: () => bound,
  }
}
