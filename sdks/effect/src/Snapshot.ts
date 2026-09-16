import { Duration, Effect, Ref, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type { DatabaseSync } from "node:sqlite"
import {
  __getUnderlyingDatabase,
  isSqliteClient,
  type SqliteClient,
} from "./Sqlite/SqliteClient.js"

/**
 * Per-agent snapshotting configuration.
 *
 * Snapshotting is opt-in. A snapshotted agent supplies separate `initialize`
 * and `restore` factories and a state {@link Strategy}. The strategy describes
 * how the initialized state is saved and how a fresh state is constructed from
 * a saved value. {@link ref} is the standard strategy for state held in an
 * Effect `Ref`.
 *
 * {@link define} uses a schema to encode and decode a plain saved value.
 * A strategy may also expose named SQLite databases from its live state; all
 * names must be declared by the definition's `databases` tuple. Use
 * {@link custom} when the saved value is already a binary payload.
 *
 * @since 1.5.0
 */

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/**
 * Snapshotting policy mirroring the WIT
 * `golem:agent/common.snapshotting-config` variant. `manual` is an
 * alias for `default` (the official TS SDK names it that way).
 *
 * @since 1.5.0
 * @category models
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

/**
 * Namespace of policy constructors used inside `Snapshot.define` /
 * `Snapshot.custom`.
 *
 * @since 1.5.0
 * @category constructors
 */
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

/**
 * Local WIT-drift exhaustiveness witness for {@link policy} +
 * {@link policyToWit}: every tag in
 * `golem:agent/common@2.0.0.snapshotting-config` must have a corresponding
 * SDK constructor. If `golem-types/*.d.ts` is regenerated with a new
 * variant, this `satisfies` clause fails to compile and points directly
 * at the wrapper that needs updating.
 */
void ({
  default: policyDefault,
  periodic: policyPeriodic,
  "every-n-invocation": policyEveryN,
} satisfies Record<AgentCommon.SnapshottingConfig["tag"], unknown>)

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

/**
 * Surfaced from `registerAgent` for malformed
 * `Snapshot.define`/`Snapshot.custom` shapes.
 *
 * @since 1.5.0
 * @category errors
 */
export class InvalidSnapshotError {
  readonly _tag = "InvalidSnapshotError"
  readonly message: string
  constructor(readonly reason: string) {
    this.message = `InvalidSnapshotError: ${reason}`
  }
}

/**
 * Raised when a snapshot strategy exposes a database name that was not
 * declared in `Snapshot.define({ databases: [...] })`.
 *
 * @since 1.5.0
 * @category errors
 */
export class SnapshotDatabaseNotDeclaredError {
  readonly _tag = "SnapshotDatabaseNotDeclaredError"
  readonly message: string
  constructor(
    readonly agentName: string,
    readonly databaseName: string,
  ) {
    this.message = `SnapshotDatabaseNotDeclaredError: agent '${agentName}' exposed database '${databaseName}' but '${databaseName}' is not listed in 'Snapshot.define({ databases: [...] })'`
  }
}

/**
 * Raised when, at save or load time, a declared database name has no
 * corresponding database exposed by the state strategy (`phase: "save"` or
 * `phase: "load-attach"`) or no corresponding
 * part in the loaded envelope (`phase: "load-envelope"`).
 *
 * @since 1.5.0
 * @category errors
 */
export class SnapshotDatabaseMissingPartError {
  readonly _tag = "SnapshotDatabaseMissingPartError"
  readonly message: string
  constructor(
    readonly agentName: string,
    readonly databaseName: string,
    readonly phase: "save" | "load-attach" | "load-envelope",
  ) {
    switch (phase) {
      case "save":
        this.message = `SnapshotDatabaseMissingPartError: agent '${agentName}' save: declared database '${databaseName}' but the snapshot strategy did not expose it`
        break
      case "load-attach":
        this.message = `SnapshotDatabaseMissingPartError: agent '${agentName}' load: declared database '${databaseName}' but the restored state's snapshot strategy did not expose it`
        break
      case "load-envelope":
        this.message = `SnapshotDatabaseMissingPartError: agent '${agentName}' load: snapshot envelope is missing required 'db:${databaseName}' part`
        break
    }
  }
}

/**
 * Raised at load time when a snapshot envelope contains a `db:<name>`
 * part whose `<name>` is not in the agent's declared `databases`
 * tuple.
 *
 * @since 1.5.0
 * @category errors
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
 *
 * @since 1.5.0
 * @category errors
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
 *
 * @since 1.5.0
 * @category errors
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
 * The schema describes the plain saved value produced by the agent's state
 * strategy. The SDK encodes that value, JSON-stringifies it, and wraps the
 * result in the JSON envelope.
 *
 * The optional `databases` tuple declares one or more SQLite databases
 * that may be exposed by the state strategy and captured alongside the auto
 * state. When `databases` is non-empty the
 * envelope on the wire becomes `multipart/mixed` with one
 * `application/x-sqlite3` part per declared database.
 *
 * @since 1.5.0
 * @category models
 */
export interface AutoSnapshotDef<S extends Schema.Top, DBs extends ReadonlyArray<string> = []> {
  readonly _tag: "AutoSnapshotDef"
  readonly schema: S
  readonly policy: SnapshotPolicy
  readonly databases?: DBs
  /** Phantom marker preserving the schema's decoded state type. */
  readonly [snapshotDefBrand]?: S["Type"]
}

/**
 * User-managed snapshot definition. Produced by {@link Snapshot.custom}.
 *
 * The agent's snapshot strategy is responsible for producing and restoring
 * the binary payload.
 *
 * @since 1.5.0
 * @category models
 */
export interface CustomSnapshotDef {
  readonly _tag: "CustomSnapshotDef"
  readonly policy: SnapshotPolicy
}

/**
 * Either flavour of snapshot definition that may appear on
 * `AgentMetadata.snapshot`.
 *
 * @since 1.5.0
 * @category models
 */
export type SnapshotDef = AutoSnapshotDef<Schema.Top, ReadonlyArray<string>> | CustomSnapshotDef

/**
 * SQLite database handle accepted from a snapshot strategy.
 *
 * @since 1.5.0
 * @category models
 */
export type AttachableDatabase = SqliteClient | DatabaseSync

/** Context supplied when restoring a fresh snapshotted agent instance. @since 1.6.0 @category models */
export interface SnapshotRestorationContext<
  Id = Readonly<Record<string, unknown>>,
  Config = unknown,
> {
  readonly id: Id
  readonly principal: AgentCommon.Principal
  readonly phantomId: CoreTypes.Uuid | undefined
  readonly agentId: CoreTypes.AgentId
  /** Complete host-parsed agent id string, including any phantom identity. */
  readonly parsedAgentId: string
  readonly config: Config
}

/**
 * State lifecycle used by a snapshotted agent's initialization, methods, and
 * restoration factory. `save` projects live state to the schema-encoded plain
 * value (or custom bytes), while `restore` constructs fresh live state. Use
 * `databases` to expose declared SQLite handles owned by the live state.
 *
 * @since 1.6.0
 * @category models
 */
export interface Strategy<
  State,
  Saved,
  R = never,
  Id = Readonly<Record<string, unknown>>,
  Config = unknown,
> {
  readonly save: (state: State) => Effect.Effect<Saved, unknown, R>
  readonly restore: (
    saved: Saved,
    context: SnapshotRestorationContext<Id, Config>,
  ) => Effect.Effect<State, unknown, R>
  readonly databases?: (state: State) => Readonly<Record<string, AttachableDatabase>>
}

/**
 * Preserve inference for a typed snapshot strategy, including its agent ID and
 * configuration available through {@link SnapshotRestorationContext}.
 *
 * @since 1.6.0
 * @category constructors
 */
export const strategy = <
  State,
  Saved,
  R = never,
  Id = Readonly<Record<string, unknown>>,
  Config = unknown,
>(
  value: Strategy<State, Saved, R, Id, Config>,
): Strategy<State, Saved, R, Id, Config> => value

/**
 * Snapshot strategy for state held in an Effect `Ref`; methods operate on the
 * `Ref`, while snapshots contain only its plain value.
 *
 * @since 1.6.0
 * @category constructors
 */
export const ref = <Saved>(): Strategy<Ref.Ref<Saved>, Saved> => ({
  save: Ref.get,
  restore: (saved) => Ref.make(saved),
})

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/**
 * Schema-driven snapshot definition for the plain value produced by the
 * agent's snapshot strategy. The initialized state itself may be richer, such
 * as an Effect `Ref`; only the strategy's saved value is encoded by `schema`.
 *
 * The optional `databases` tuple — typically declared as
 * `databases: ["counters"] as const` — pre-declares one or more
 * SQLite databases that the strategy may expose from initialized or restored
 * state. Save and load continue to reject a missing declared database.
 *
 * @since 1.5.0
 * @category constructors
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
 * Binary snapshot definition for a strategy that directly saves and restores
 * `Uint8Array` values.
 *
 * @since 1.5.0
 * @category constructors
 */
export const custom = (spec: { readonly policy: SnapshotPolicy }): CustomSnapshotDef => ({
  _tag: "CustomSnapshotDef",
  policy: spec.policy,
})

// ---------------------------------------------------------------------------
// Compiled bundle (consumed by agent.ts)
// ---------------------------------------------------------------------------

/**
 * Compiled view of the user's snapshot definition.
 *
 * @since 1.5.0
 * @category models
 */
export type CompiledSnapshot =
  | {
      readonly kind: "auto"
      readonly policy: SnapshotPolicy
      readonly witConfig: AgentCommon.SnapshottingConfig
      readonly schema: Schema.Top
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
 *
 * @since 1.5.0
 * @category metadata
 */
export const compileSnapshot = (
  agentName: string,
  def: SnapshotDef,
): Effect.Effect<CompiledSnapshot, InvalidSnapshotError> =>
  Effect.gen(function* () {
    const witConfig = yield* policyToWit(def.policy, `agent '${agentName}' snapshot`)
    if (def._tag === "AutoSnapshotDef") {
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
        declaredDatabases: [...rawDbs],
      }
    }
    return { kind: "custom", policy: def.policy, witConfig }
  })

// ---------------------------------------------------------------------------
// Per-instance snapshot state
// ---------------------------------------------------------------------------

/**
 * Runtime snapshot resources resolved from a compiled definition and the
 * databases exposed by an initialized or restored state.
 *
 * @since 1.5.0
 * @category models
 */
export type BoundSnapshot =
  | {
      readonly kind: "auto"
      readonly schema: Schema.Top
      readonly declaredDatabases: ReadonlyArray<string>
      readonly databases: ReadonlyMap<string, DatabaseSync>
    }
  | { readonly kind: "custom" }

/**
 * Resolve the SQLite handles exposed by an initialized or restored state.
 * Unknown names are rejected immediately; declared names omitted here remain
 * subject to the save/load missing-part checks.
 *
 * @since 1.6.0
 * @category constructors
 */
export const createSnapshot = (
  agentName: string,
  compiled: CompiledSnapshot,
  databases: Readonly<Record<string, AttachableDatabase>> = {},
): BoundSnapshot => {
  if (compiled.kind === "custom") {
    const unknownName = Object.keys(databases)[0]
    if (unknownName !== undefined) {
      throw new SnapshotDatabaseNotDeclaredError(agentName, unknownName)
    }
    return { kind: "custom" }
  }

  const declared = new Set(compiled.declaredDatabases)
  const resolved = new Map<string, DatabaseSync>()
  for (const [name, database] of Object.entries(databases)) {
    if (!declared.has(name)) {
      throw new SnapshotDatabaseNotDeclaredError(agentName, name)
    }
    resolved.set(name, isSqliteClient(database) ? __getUnderlyingDatabase(database) : database)
  }
  return {
    kind: "auto",
    schema: compiled.schema,
    declaredDatabases: compiled.declaredDatabases,
    databases: resolved,
  }
}

// ---------------------------------------------------------------------------
// Re-exports from the snapshot envelope codec.
//
// The envelope encoder/decoder (JSON / binary v2 / multipart-mixed for SQLite
// databases) lives in `src/internal/snapshotEnvelope.ts` because consumers
// never construct envelopes directly — that is the SDK dispatcher's job. The
// two error classes it raises, however, are part of the public `Snapshot.*`
// namespace contract: any `dispatchLoadSnapshot` failure surfaces one of them.
//
// @since 1.5.0
// ---------------------------------------------------------------------------

/**
 * @since 1.5.0
 * @category errors
 */
export {
  SnapshotEnvelopeError,
  UnsupportedSnapshotFormatError,
} from "./internal/snapshotEnvelope.js"
