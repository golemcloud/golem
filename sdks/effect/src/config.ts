import { Context, Effect, Redacted, Schema, SchemaAST } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import * as AgentHost from "golem:agent/host@1.5.0"
import { toWitCodec, UnsupportedSchemaError, type WitCodec } from "./wit-codec.js"

type WitType = CoreTypes.WitType
type WitValue = CoreTypes.WitValue

/**
 * A typed config-fetching failure surfaced through the {@link ConfigError}
 * Effect channel. Mirrors the rest of effect-golem's "errors as Effect
 * typed failures" convention (avoid throwing in Effect bodies).
 */
export class ConfigError {
  readonly _tag = "ConfigError"
  constructor(
    readonly path: ReadonlyArray<string>,
    readonly reason:
      | { _tag: "HostTrap"; cause: unknown }
      | { _tag: "DecodeFailure"; cause: Schema.SchemaError }
      | { _tag: "WireMismatch"; got: string; expected: string }
      | { _tag: "Unsupported"; reason: string },
  ) {}
}

/**
 * Allowed shapes for a single named entry in a config schema record:
 * - a `Schema.Top` (becomes a "local" leaf, accessed as
 *   `Effect<T, ConfigError>`),
 * - `Schema.Redacted(inner)` (becomes a "secret" leaf, accessed as
 *   `{ get: Effect<Redacted<T>, ConfigError> }`),
 * - or a literal `Schema.Struct(...)` (recursed into, prefixing the
 *   path with the field's name).
 */
export type ConfigField = Schema.Top

/** Record of named config fields, supplied to {@link defineConfig}. */
export type ConfigFields = Readonly<Record<string, ConfigField>>

/**
 * Recursively map a {@link ConfigFields} record to its decoded
 * "config shape". See {@link ConfigError} for the failure channel.
 */
export type ConfigShape<F extends ConfigFields> = {
  readonly [K in keyof F]: F[K] extends Schema.Redacted<infer Inner>
    ? { readonly get: Effect.Effect<Redacted.Redacted<Inner["Type"]>, ConfigError> }
    : F[K] extends Schema.Struct<infer SF>
      ? SF extends ConfigFields
        ? ConfigShape<SF>
        : never
      : Effect.Effect<F[K]["Type"], ConfigError>
}

/**
 * Recursive `Partial<>` over a {@link ConfigFields} record with all
 * `Schema.Redacted(...)` leaves stripped. Used by {@link AgentClient}
 * `GetOptions.overrides` so callers cannot accidentally provide a
 * secret leaf via RPC overrides at compile time (a runtime guard
 * enforces the same invariant).
 */
export type NonSecretOverride<F extends ConfigFields> = {
  readonly [K in keyof F as F[K] extends Schema.Redacted<any>
    ? never
    : K]?: F[K] extends Schema.Struct<infer SF>
    ? SF extends ConfigFields
      ? NonSecretOverride<SF>
      : never
    : F[K] extends Schema.Top
      ? F[K]["Type"]
      : never
}

/**
 * Module-level indirection so tests can swap the host's `getConfigValue`
 * binding without monkey-patching the imported namespace.
 */
let getConfigValueImpl: (key: Array<string>, expectedType: WitType) => WitValue = (
  key,
  expectedType,
) => AgentHost.getConfigValue(key, expectedType)

/** Test-only hook: replace the host `getConfigValue` shim. */
export const __setGetConfigValueForTest = (
  fn: (key: Array<string>, expectedType: WitType) => WitValue,
): void => {
  getConfigValueImpl = fn
}

/** Reset the host shim back to the real `golem:agent/host@1.5.0` binding. */
export const __resetGetConfigValueForTest = (): void => {
  getConfigValueImpl = (key, expectedType) => AgentHost.getConfigValue(key, expectedType)
}

/** A single compiled leaf inside a config schema. */
interface ConfigLeaf {
  readonly source: AgentCommon.AgentConfigSource
  readonly path: ReadonlyArray<string>
  readonly witCodec: WitCodec<Schema.Top>
}

/** Compiled bundle held alongside the {@link defineConfig}-class metadata. */
export interface CompiledConfig {
  readonly declarations: ReadonlyArray<AgentCommon.AgentConfigDeclaration>
  readonly leaves: ReadonlyArray<ConfigLeaf>
  /** Lookup by `path.join("/")` for runtime override encoding/validation. */
  readonly leavesByPath: ReadonlyMap<string, ConfigLeaf>
  /**
   * Set of `path.join("/")` strings naming every nested `Schema.Struct`
   * branch. Used by the runtime to materialise empty branches in the
   * config shape and to validate `overrides` paths that descend through
   * intermediate objects.
   */
  readonly branches: ReadonlySet<string>
  readonly buildShape: () => Effect.Effect<unknown>
}

/** Detect `Schema.Redacted(inner)` by walking down to the underlying AST. */
const declarationConstructorTag = (a: SchemaAST.AST): string | undefined => {
  if (a._tag !== "Declaration") return undefined
  const tc = (a.annotations as { typeConstructor?: { _tag?: string } } | undefined)?.typeConstructor
  return tc?._tag
}

const isRedactedSchema = (s: Schema.Top): s is Schema.Redacted<Schema.Top> =>
  declarationConstructorTag(s.ast) === "effect/Redacted"

const isPlainStructSchema = (s: Schema.Top): s is Schema.Struct<ConfigFields> => {
  const a = s.ast
  if (!SchemaAST.isObjects(a)) return false
  if (a.indexSignatures.length !== 0) return false
  // All property names must be strings (configs cannot be keyed by symbols).
  return a.propertySignatures.every((ps) => typeof ps.name === "string")
}

/**
 * Walk the user-supplied config record. Produces a flat list of leaves
 * (each with its WIT type, decoder, and path) plus the matching
 * AgentConfigDeclaration array used during agent registration.
 */
export const compileConfig = (
  fields: ConfigFields,
  contextLabel: string,
): Effect.Effect<CompiledConfig, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    const leaves: Array<ConfigLeaf> = []
    const branches = new Set<string>()

    const visit = (
      schema: Schema.Top,
      path: ReadonlyArray<string>,
    ): Effect.Effect<void, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        // Secret leaf: Schema.Redacted(inner). The host stores the raw
        // inner value; the wrapper is purely a guest-side hygiene
        // concern. Use the inner schema for the WitType + decoder.
        if (isRedactedSchema(schema)) {
          // The Declaration AST stores its type parameters under
          // `typeParameters` — `Schema.Redacted` puts the inner schema
          // there. (The user-facing `.value` accessor on the schema
          // object is not a guaranteed runtime path on every Effect
          // version, so we walk the AST directly.)
          const innerAst = (schema.ast as SchemaAST.Declaration).typeParameters[0]
          if (innerAst === undefined) {
            return yield* Effect.fail(
              new UnsupportedSchemaError(
                `${contextLabel}: ${path.join(".") || "<root>"}: Schema.Redacted without inner schema`,
              ),
            )
          }
          const inner = Schema.make<Schema.Top>(innerAst as Schema.Top["ast"])
          const witCodec = (yield* toWitCodec(inner)) as WitCodec<Schema.Top>
          leaves.push({ source: "secret", path, witCodec })
          return
        }

        // Recurse into nested Structs: each property becomes its own
        // leaf path prefixed by the parent field name. Record the
        // branch so empty structs still appear in the materialised
        // shape and `encodeOverrides` can validate descent.
        if (isPlainStructSchema(schema)) {
          if (path.length > 0) branches.add(path.join("/"))
          const obj = schema.ast as SchemaAST.Objects
          for (const ps of obj.propertySignatures) {
            const name = ps.name as string
            const childSchema = Schema.make(ps.type) as Schema.Top
            yield* visit(childSchema, [...path, name])
          }
          return
        }

        // Local leaf: any other Schema.Top reachable via toWitCodec.
        const witCodec = (yield* toWitCodec(schema)) as WitCodec<Schema.Top>
        leaves.push({ source: "local", path, witCodec })
      })

    for (const [name, field] of Object.entries(fields)) {
      yield* visit(field, [name])
    }

    const declarations: Array<AgentCommon.AgentConfigDeclaration> = leaves.map((leaf) => ({
      source: leaf.source,
      path: [...leaf.path],
      valueType: leaf.witCodec.witType,
    }))
    const leavesByPath = new Map<string, ConfigLeaf>(
      leaves.map((leaf) => [leaf.path.join("/"), leaf] as const),
    )

    /**
     * Walk down `path` inside `root`, materialising plain object nodes
     * as needed. Returns the deepest object so a caller can attach a
     * leaf value at the final segment.
     */
    const ensureBranch = (
      root: Record<string, unknown>,
      path: ReadonlyArray<string>,
    ): Record<string, unknown> => {
      let cursor: Record<string, unknown> = root
      for (const segment of path) {
        const next = cursor[segment]
        if (next === undefined || typeof next !== "object" || next === null) {
          const created: Record<string, unknown> = {}
          cursor[segment] = created
          cursor = created
        } else {
          cursor = next as Record<string, unknown>
        }
      }
      return cursor
    }

    const buildShape: () => Effect.Effect<unknown> = () =>
      Effect.gen(function* () {
        const root: Record<string, unknown> = {}

        // Materialise empty struct branches first so a `database:
        // Schema.Struct({})` field still appears as `cfg.database = {}`
        // in the shape, even when no leaves live underneath it. Sort
        // shortest-first so parents are created before children.
        for (const branch of [...branches].sort(
          (a, b) => a.split("/").length - b.split("/").length,
        )) {
          ensureBranch(root, branch.split("/"))
        }

        for (const leaf of leaves) {
          const fetch: Effect.Effect<unknown, ConfigError> = Effect.gen(function* () {
            const wv = yield* Effect.try({
              try: () => getConfigValueImpl([...leaf.path], leaf.witCodec.witType),
              catch: (cause) => new ConfigError(leaf.path, { _tag: "HostTrap", cause }),
            })
            const decoded = yield* Effect.mapError(
              Schema.decodeEffect(
                leaf.witCodec.codec as Schema.Codec<unknown, WitValue, never, never>,
              )(wv) as Effect.Effect<unknown, Schema.SchemaError>,
              (cause) =>
                new ConfigError(leaf.path, {
                  _tag: "DecodeFailure",
                  cause,
                }),
            )
            return decoded
          })

          const value =
            leaf.source === "secret"
              ? {
                  get: Effect.map(fetch, (raw) => Redacted.make(raw, undefined)) as Effect.Effect<
                    Redacted.Redacted<unknown>,
                    ConfigError
                  >,
                }
              : // Per-invocation memo: Effect.cached returns
                // Effect<Effect<…>>; running it eagerly here yields the
                // memoized inner effect bound to this shape. Two reads
                // inside one invocation share a single host call; a
                // fresh shape is built for each dispatch entry, so
                // subsequent invocations see fresh values.
                yield* Effect.cached(fetch)

          // Place the value at `leaf.path` inside the recursive `root`.
          const parent = ensureBranch(root, leaf.path.slice(0, -1))
          parent[leaf.path[leaf.path.length - 1]!] = value
        }

        return root
      })

    return { declarations, leaves, leavesByPath, branches, buildShape }
  })

/**
 * Static metadata attached to a `defineConfig`-class so
 * {@link AgentDefinition} (and the runtime dispatcher) can discover the
 * compiled bundle, build per-invocation shapes, and type the
 * `overrides` channel of {@link AgentClient.GetOptions}.
 */
export interface ConfigStatics<F extends ConfigFields> {
  readonly fields: F
  /**
   * Lazy single-shot compile cell. The first call materialises the
   * compiled bundle (declarations + buildShape) and caches it; later
   * calls reuse the same bundle.
   */
  readonly __compile: () => Effect.Effect<CompiledConfig, UnsupportedSchemaError>
  /**
   * Phantom marker used by `defineAgent` to anchor inference of the
   * agent's `CfgTag` generic. Carries the {@link ConfigShape} so that
   * `yield* MyConfig` plays nicely with `Effect.provideService(MyConfig,
   * shape)` (both reduce/refer to the same R-slot identity).
   */
  readonly __cfgTag: ConfigShape<F>
  /**
   * Phantom marker used by `clientFor` to recover the original fields
   * record (so it can derive `NonSecretOverride<F>` for `GetOptions.overrides`).
   */
  readonly __fields: F
}

/**
 * Marker class type returned by {@link defineConfig}.
 *
 * Concrete users do
 *
 * ```ts
 * class CounterConfig extends defineConfig("Counter.Config", { ... }) {}
 * ```
 *
 * which makes `CounterConfig` simultaneously a `Context.Service` tag
 * (yieldable to its {@link ConfigShape}) AND a constructor carrying the
 * static `fields` / `__compile` metadata.
 *
 * The Self parameter of the underlying `Context.Service` is left as
 * `any` so that user subclasses (the typical extension pattern shown
 * above) remain assignable; the runtime tag identity is the unique
 * `KeyClass` minted by `Context.Service<...>(name)` per call.
 */
export type ConfigClass<F extends ConfigFields> = Context.ServiceClass<
  ConfigShape<F>,
  string,
  ConfigShape<F>
> &
  ConfigStatics<F>

/**
 * Build a `Context.Service` class whose payload is the recursively
 * mapped {@link ConfigShape} of the supplied fields.
 *
 * The returned class also carries (as static members) the original
 * `fields` record and a memoized `__compile` accessor so
 * {@link defineAgent} / {@link clientFor} can produce the matching
 * AgentConfigDeclarations and runtime shapes without re-walking the
 * schema.
 */
export const defineConfig = <const F extends ConfigFields>(
  name: string,
  fields: F,
): ConfigClass<F> => {
  // Self = ConfigShape<F> so that `yield* MyConfig` adds exactly
  // `ConfigShape<F>` to the effect's R slot, which matches the
  // `CfgTag = ConfigShape<F>` inferred by `defineAgent`. The runtime
  // tag identity is still the unique `KeyClass` minted below.
  class Base extends Context.Service<ConfigShape<F>, ConfigShape<F>>()(name) {
    static readonly fields: F = fields
    /**
     * Phantom static — never read at runtime; exists only so
     * `defineAgent` can infer its `CfgTag` generic from
     * `def.config.__cfgTag`.
     */
    static readonly __cfgTag: ConfigShape<F> = undefined as never
    /**
     * Phantom static — never read at runtime; exists only so
     * `clientFor` can recover the fields record (and from it the
     * `NonSecretOverride<F>` type) by inferring it from `def.config.__fields`.
     */
    static readonly __fields: F = fields

    private static cached: CompiledConfig | null = null

    static __compile(): Effect.Effect<CompiledConfig, UnsupportedSchemaError> {
      return Effect.suspend(() => {
        if (Base.cached !== null) return Effect.succeed(Base.cached)
        return Effect.tap(compileConfig(fields, name), (cc) => {
          Base.cached = cc
          return Effect.void
        })
      })
    }
  }
  return Base as unknown as ConfigClass<F>
}

/**
 * Encode a non-secret override record into the `TypedAgentConfigValue[]`
 * shape consumed by the WasmRpc constructor.
 *
 * Walks the override object alongside the compiled leaf table so that:
 * - any leaf marked as `secret` is rejected with a clear error (defense
 *   in depth on top of the type-level {@link NonSecretOverride} guard);
 * - missing keys are skipped (overrides are always partial);
 * - extra keys are surfaced as an {@link UnsupportedSchemaError}.
 *
 * Returns the encoded array; failure paths surface as
 * {@link UnsupportedSchemaError} or {@link ConfigError}.
 */
export const encodeOverrides = (
  compiled: CompiledConfig,
  overrides: Record<string, unknown>,
): Effect.Effect<Array<AgentCommon.TypedAgentConfigValue>, ConfigError | Schema.SchemaError> =>
  Effect.gen(function* () {
    const out: Array<AgentCommon.TypedAgentConfigValue> = []

    const visit = (
      value: unknown,
      path: Array<string>,
    ): Effect.Effect<void, ConfigError | Schema.SchemaError> =>
      Effect.gen(function* () {
        const key = path.join("/")
        const leaf = compiled.leavesByPath.get(key)

        if (leaf === undefined) {
          // Only descend if `path` actually names a recorded struct
          // branch in the compiled config — otherwise an arbitrary
          // `{ unknownKey: {} }` would silently no-op. Empty objects
          // at unknown paths are treated as errors too.
          if (
            compiled.branches.has(key) &&
            typeof value === "object" &&
            value !== null &&
            !Array.isArray(value)
          ) {
            for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
              yield* visit(v, [...path, k])
            }
            return
          }
          return yield* Effect.fail(
            new ConfigError(path, {
              _tag: "Unsupported",
              reason: `unknown override path: ${path.join(".")}`,
            }),
          )
        }

        if (leaf.source === "secret") {
          return yield* Effect.fail(
            new ConfigError(path, {
              _tag: "Unsupported",
              reason: `cannot override secret config path: ${path.join(".")}`,
            }),
          )
        }

        const wv = yield* Schema.encodeEffect(
          leaf.witCodec.codec as Schema.Codec<unknown, WitValue, never, never>,
        )(value) as Effect.Effect<WitValue, Schema.SchemaError>
        out.push({
          path: [...leaf.path],
          value: { value: wv, typ: leaf.witCodec.witType },
        })
      })

    for (const [name, value] of Object.entries(overrides)) {
      yield* visit(value, [name])
    }

    return out
  })
