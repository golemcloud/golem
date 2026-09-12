import { Context, Effect, Redacted, Schema, SchemaAST } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { ConfigClient } from "./host/ConfigClient.js"
import { SecretsClient } from "./host/SecretsClient.js"
import { t, type SchemaGraph } from "./internal/schema-model/model.js"
import { schemaGraphToWit } from "./internal/schema-model/wit.js"
import { compile, UnsupportedSchemaError, type CompiledWitCodec } from "./WitCodec.js"

export class ConfigError {
  readonly _tag = "ConfigError"
  constructor(
    readonly path: ReadonlyArray<string>,
    readonly reason:
      | { _tag: "HostTrap"; cause: unknown }
      | { _tag: "DecodeFailure"; cause: unknown }
      | { _tag: "Unsupported"; reason: string },
  ) {}
}

export type ConfigField = Schema.Top
export type ConfigFields = Readonly<Record<string, ConfigField>>
type OptionalValue<A, Optional extends boolean> = Optional extends true ? A | undefined : A
type ConfigShapeField<S extends Schema.Top, Optional extends boolean = false> =
  S extends Schema.optional<infer Inner>
    ? Inner extends Schema.Top
      ? ConfigShapeField<Inner, true>
      : never
    : S extends Schema.Redacted<infer Inner>
      ? {
          readonly get: Effect.Effect<
            Redacted.Redacted<OptionalValue<Inner["Type"], Optional>>,
            ConfigError
          >
        }
      : S extends Schema.Struct<infer SF>
        ? SF extends ConfigFields
          ? ConfigShape<SF, Optional>
          : Effect.Effect<OptionalValue<S["Type"], Optional>, ConfigError>
        : Effect.Effect<OptionalValue<S["Type"], Optional>, ConfigError>
export type ConfigShape<F extends ConfigFields, Optional extends boolean = false> = {
  readonly [K in keyof F]: ConfigShapeField<F[K], Optional>
}
export type NonSecretOverride<F extends ConfigFields> = {
  readonly [K in keyof F as F[K] extends Schema.Redacted<any>
    ? never
    : K]?: F[K] extends Schema.Struct<infer SF>
    ? SF extends ConfigFields
      ? NonSecretOverride<SF>
      : never
    : F[K]["Type"]
}

export interface ConfigLeaf {
  readonly source: AgentCommon.AgentConfigSource
  readonly path: ReadonlyArray<string>
  readonly codec: CompiledWitCodec<Schema.Top>
  readonly declarationGraph: SchemaGraph
  readonly required: boolean
}

export interface CompiledConfig {
  /** Graph roots to merge with constructor and method graphs, in declaration order. */
  readonly graphs: ReadonlyArray<SchemaGraph>
  readonly leaves: ReadonlyArray<ConfigLeaf>
  readonly leavesByPath: ReadonlyMap<string, ConfigLeaf>
  readonly branches: ReadonlySet<string>
  /** Build declarations after graph merging by supplying indices in `graphs` order. */
  readonly declarations: (
    valueTypeIndices: ReadonlyArray<number>,
  ) => ReadonlyArray<AgentCommon.AgentConfigDeclaration>
  readonly buildShape: () => Effect.Effect<unknown, never, ConfigClient | SecretsClient>
}

const redactedInner = (schema: Schema.Top): Schema.Top | undefined => {
  const ast = schema.ast
  if (ast._tag !== "Declaration") return undefined
  const tag = (ast.annotations as { typeConstructor?: { _tag?: string } } | undefined)
    ?.typeConstructor?._tag
  return tag === "effect/Redacted" && ast.typeParameters[0] !== undefined
    ? (Schema.make(ast.typeParameters[0]) as Schema.Top)
    : undefined
}

const objectAst = (
  ast: SchemaAST.AST,
): { ast: SchemaAST.Objects; optional: boolean } | undefined => {
  if (SchemaAST.isObjects(ast) && ast.indexSignatures.length === 0) return { ast, optional: false }
  if (ast._tag === "Union" && ast.context?.isOptional === true) {
    const object = ast.types.find(SchemaAST.isObjects)
    if (object !== undefined && object.indexSignatures.length === 0)
      return { ast: object, optional: true }
  }
  return undefined
}

export const compileConfig = (
  fields: ConfigFields,
  contextLabel: string,
): Effect.Effect<CompiledConfig, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    const leaves: ConfigLeaf[] = []
    const branches = new Set<string>()
    const visit = (
      schema: Schema.Top,
      path: string[],
      underOptional: boolean,
      required: boolean,
    ): Effect.Effect<void, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        const inner = redactedInner(schema)
        if (inner !== undefined) {
          const valueSchema = underOptional ? Schema.UndefinedOr(inner) : inner
          const codec = (yield* compile(valueSchema)) as CompiledWitCodec<Schema.Top>
          const declarationGraph = { defs: codec.graph.defs, root: t.secret(codec.graph.root) }
          leaves.push({ source: "secret", path, codec, declarationGraph, required: true })
          return
        }
        const object = objectAst(schema.ast)
        if (object !== undefined) {
          branches.add(path.join("/"))
          for (const property of object.ast.propertySignatures) {
            if (typeof property.name !== "string") {
              return yield* Effect.fail(
                new UnsupportedSchemaError(`${contextLabel}: config object keys must be strings`),
              )
            }
            const optional = property.type.context?.isOptional === true
            yield* visit(
              Schema.make(property.type),
              [...path, property.name],
              underOptional || object.optional,
              !optional,
            )
          }
          return
        }
        let codec = (yield* compile(schema)) as CompiledWitCodec<Schema.Top>
        if (underOptional && codec.graph.root.body.tag !== "option") {
          const optionalSchema = Schema.UndefinedOr(schema)
          codec = (yield* compile(optionalSchema)) as unknown as CompiledWitCodec<Schema.Top>
        }
        leaves.push({
          source: "local",
          path,
          codec,
          declarationGraph: codec.graph,
          required,
        })
      })
    for (const [name, schema] of Object.entries(fields)) yield* visit(schema, [name], false, true)
    const leavesByPath = new Map(leaves.map((leaf) => [leaf.path.join("/"), leaf]))
    const ensure = (root: Record<string, unknown>, path: ReadonlyArray<string>) => {
      let cursor = root
      for (const segment of path) {
        cursor = (cursor[segment] ??= {}) as Record<string, unknown>
      }
      return cursor
    }
    return {
      graphs: leaves.map((leaf) => leaf.declarationGraph),
      leaves,
      leavesByPath,
      branches,
      declarations: (indices) => {
        if (indices.length !== leaves.length)
          throw new Error("config declaration index count mismatch")
        return leaves.map((leaf, i) => ({
          source: leaf.source,
          path: [...leaf.path],
          valueType: indices[i]!,
        }))
      },
      buildShape: () =>
        Effect.gen(function* () {
          const config = yield* ConfigClient
          const secrets = leaves.some((leaf) => leaf.source === "secret")
            ? yield* SecretsClient
            : undefined
          const root: Record<string, unknown> = {}
          for (const branch of branches) ensure(root, branch.split("/"))
          for (const leaf of leaves) {
            const read = Effect.gen(function* () {
              const tree = yield* Effect.try({
                try: () =>
                  config.getConfigValue(leaf.path, schemaGraphToWit(leaf.declarationGraph)),
                catch: (cause) => new ConfigError(leaf.path, { _tag: "HostTrap", cause }),
              })
              if (leaf.source === "secret") {
                const value = yield* Effect.mapError(
                  leaf.codec.decode(tree),
                  (cause) => new ConfigError(leaf.path, { _tag: "DecodeFailure", cause }),
                )
                return Redacted.make(value)
              }
              return yield* Effect.mapError(
                leaf.codec.decode(tree),
                (cause) => new ConfigError(leaf.path, { _tag: "DecodeFailure", cause }),
              )
            })
            const value =
              leaf.source === "secret"
                ? {
                    get: Effect.gen(function* () {
                      const tree = yield* Effect.try({
                        try: () =>
                          config.getConfigValue(leaf.path, schemaGraphToWit(leaf.declarationGraph)),
                        catch: (cause) => new ConfigError(leaf.path, { _tag: "HostTrap", cause }),
                      })
                      const sv = leaf.codec.codec // keep inner codec separate from capability graph
                      const rawTree = tree as CoreTypes.SchemaValueTree
                      const node = rawTree.valueNodes[rawTree.root]
                      const raw = node?.tag === "secret-value" ? node.val : undefined
                      if (raw === undefined) {
                        return yield* Effect.fail(
                          new ConfigError(leaf.path, {
                            _tag: "Unsupported",
                            reason: "expected secret handle",
                          }),
                        )
                      }
                      void sv
                      const revealed = yield* Effect.try({
                        try: () => secrets!.reveal(raw, leaf.codec.schemaGraph),
                        catch: (cause) => new ConfigError(leaf.path, { _tag: "HostTrap", cause }),
                      })
                      const decoded = yield* Effect.mapError(
                        leaf.codec.decode(revealed),
                        (cause) => new ConfigError(leaf.path, { _tag: "DecodeFailure", cause }),
                      )
                      return Redacted.make(decoded)
                    }),
                  }
                : yield* Effect.cached(read)
            ensure(root, leaf.path.slice(0, -1))[leaf.path.at(-1)!] = value
          }
          return root
        }),
    }
  })

export interface ConfigStatics<F extends ConfigFields> {
  readonly fields: F
  readonly __compile: () => Effect.Effect<CompiledConfig, UnsupportedSchemaError>
  readonly __cfgTag: ConfigShape<F>
  readonly __fields: F
}
export type ConfigClass<F extends ConfigFields> = Context.ServiceClass<
  ConfigShape<F>,
  string,
  ConfigShape<F>
> &
  ConfigStatics<F>

export const defineConfig = <const F extends ConfigFields>(
  name: string,
  fields: F,
): ConfigClass<F> => {
  class Base extends Context.Service<ConfigShape<F>, ConfigShape<F>>()(name) {
    static readonly fields = fields
    static readonly __cfgTag: ConfigShape<F> = undefined as never
    static readonly __fields = fields
    private static cached: CompiledConfig | null = null
    static __compile() {
      return Effect.suspend(() =>
        Base.cached === null
          ? Effect.tap(compileConfig(fields, name), (value) =>
              Effect.sync(() => (Base.cached = value)),
            )
          : Effect.succeed(Base.cached),
      )
    }
  }
  return Base as unknown as ConfigClass<F>
}

export const encodeOverrides = (
  compiled: CompiledConfig,
  overrides: Record<string, unknown>,
): Effect.Effect<AgentCommon.TypedAgentConfigValue[], ConfigError | Schema.SchemaError> =>
  Effect.gen(function* () {
    const out: AgentCommon.TypedAgentConfigValue[] = []
    const visit = (
      value: unknown,
      path: string[],
    ): Effect.Effect<void, ConfigError | Schema.SchemaError, unknown> =>
      Effect.gen(function* () {
        const key = path.join("/")
        const leaf = compiled.leavesByPath.get(key)
        if (leaf === undefined) {
          if (compiled.branches.has(key) && typeof value === "object" && value !== null) {
            for (const [name, child] of Object.entries(value)) yield* visit(child, [...path, name])
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
        out.push({
          path: [...path],
          value: {
            graph: leaf.codec.schemaGraph,
            value: yield* leaf.codec.encode(value) as Effect.Effect<
              CoreTypes.SchemaValueTree,
              Schema.SchemaError
            >,
          },
        })
      })
    for (const [name, value] of Object.entries(overrides)) yield* visit(value, [name])
    return out
  }) as Effect.Effect<AgentCommon.TypedAgentConfigValue[], ConfigError | Schema.SchemaError>
