import { Context, Effect, Layer, type Redacted, Schema, SchemaAST } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type { Secret } from "./Capability.js"
import { ConfigClient } from "./host/ConfigClient.js"
import { SecretsClient } from "./host/SecretsClient.js"
import { t, type SchemaGraph } from "./internal/schema-model/model.js"
import { schemaGraphToWit } from "./internal/schema-model/wit.js"
import { compile, UnsupportedSchemaError, type CompiledWitCodec } from "./WitCodec.js"
import { compiledConfigRuntime, type WireConfigLeaf } from "./internal/compiledConfig.js"
import { configGraph, configName } from "./internal/configSchema.js"
import { SchemaRef } from "./SchemaRef.js"

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

/** A whole static value captured by the host for its owner. @since 1.6.0 @category models */
export interface Static<S extends Schema.Top> {
  readonly _tag: "Static"
  readonly schema: S
  readonly defaultValue?: S["Type"]
}

/** Declare static data and an optional portable code default, never a read fallback.
 * @since 1.6.0 @category constructors
 */
const staticField = <S extends Schema.Top>(
  schema: S,
  options?: { readonly defaultValue: S["Type"] },
): Static<S> => ({ _tag: "Static", schema, ...options })

export { staticField as static }

export type ConfigField = Schema.Top | Static<Schema.Top>
export type ConfigFields = Readonly<Record<string, ConfigField>>
type OptionalValue<A, Optional extends boolean> = Optional extends true ? A | undefined : A
type ConfigShapeField<S extends ConfigField, Optional extends boolean = false> =
  S extends Static<infer Inner>
    ? Effect.Effect<Inner["Type"], ConfigError>
    : S extends Schema.optional<infer Inner>
      ? Inner extends Schema.Top
        ? ConfigShapeField<Inner, true>
        : never
      : S extends Schema.Redacted<infer Inner>
        ? {
            /** Fresh opaque capability without revealing its value. @since 1.6.0 @category secrets */
            readonly borrow: Effect.Effect<Secret, ConfigError>
            readonly get: Effect.Effect<
              Redacted.Redacted<OptionalValue<Inner["Type"], Optional>>,
              ConfigError
            >
          }
        : S extends Schema.Struct<infer SF>
          ? SF extends ConfigFields
            ? ConfigShape<SF, Optional>
            : S extends Schema.Top
              ? Effect.Effect<OptionalValue<S["Type"], Optional>, ConfigError>
              : never
          : S extends Schema.Top
            ? Effect.Effect<OptionalValue<S["Type"], Optional>, ConfigError>
            : never
export type ConfigShape<F extends ConfigFields, Optional extends boolean = false> = {
  readonly [K in keyof F]: ConfigShapeField<F[K], Optional>
}
type UnwrapOptional<S extends Schema.Top> =
  S extends Schema.optional<infer Inner> ? (Inner extends Schema.Top ? Inner : S) : S
type ConfigFieldsOf<S extends Schema.Top> =
  UnwrapOptional<S> extends {
    readonly fields: infer Fields
  }
    ? Fields
    : never
type IsRedacted<S extends Schema.Top> =
  UnwrapOptional<S> extends { readonly value: Schema.Top } ? true : false
type OverrideField<S extends ConfigField> =
  S extends Static<infer Inner>
    ? Inner["Type"]
    : S extends Schema.Top
      ? IsRedacted<S> extends true
        ? never
        : [ConfigFieldsOf<S>] extends [never]
          ? S["Type"]
          : ConfigFieldsOf<S> extends ConfigFields
            ? NonSecretOverride<ConfigFieldsOf<S>>
            : never
      : never
type OverrideKey<F extends ConfigFields> = {
  readonly [K in keyof F]: OverrideField<F[K]> extends never ? never : K
}[keyof F]
export type NonSecretOverride<F extends ConfigFields> = [OverrideKey<F>] extends [never]
  ? Readonly<Record<string, never>>
  : {
      readonly [K in OverrideKey<F>]?: OverrideField<F[K]>
    }

export interface ConfigLeaf {
  readonly source: AgentCommon.AgentConfigSource
  readonly path: ReadonlyArray<string>
  readonly propertyPath: ReadonlyArray<string>
  readonly schema: Schema.Top
  readonly codec: CompiledWitCodec<Schema.Top>
  readonly declarationGraph: SchemaGraph
  readonly required: boolean
  readonly static?: boolean
  readonly defaultValue?: CoreTypes.SchemaValueTree
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
  const id = (ast.annotations as { representation?: { id?: string } } | undefined)?.representation
    ?.id
  return id === "effect/schema/Redacted" && ast.typeParameters[0] !== undefined
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

const validateStaticSchema = (root: SchemaAST.AST, context: string): void => {
  const seen = new Set<SchemaAST.AST>()
  const visit = (ast: SchemaAST.AST): void => {
    if (seen.has(ast)) return
    seen.add(ast)
    if (ast.context?.constructorDefault !== undefined || ast.encoding !== undefined) {
      throw new UnsupportedSchemaError(
        `${context}: use Config.static(schema, { defaultValue }) for portable defaults, not schema factories or transformations`,
      )
    }
    switch (ast._tag) {
      case "Objects":
        for (const property of ast.propertySignatures) {
          if (typeof property.name === "string" && property.name.startsWith("$")) {
            throw new UnsupportedSchemaError(`${context}: reserved field '${property.name}'`)
          }
          visit(property.type)
        }
        for (const index of ast.indexSignatures) visit(index.type)
        break
      case "Arrays":
        for (const child of [...ast.elements, ...ast.rest]) visit(child)
        break
      case "Union":
        for (const child of ast.types) visit(child)
        break
      case "Declaration":
        for (const child of ast.typeParameters) visit(child)
        break
      case "Suspend":
        visit(ast.thunk())
        break
    }
  }
  visit(root)
}

export const compileConfig = (
  fields: ConfigFields,
  contextLabel: string,
): Effect.Effect<CompiledConfig, UnsupportedSchemaError> =>
  Effect.gen(function* () {
    const leaves: ConfigLeaf[] = []
    const branches = new Set<string>()
    const paths = new Set<string>()
    const visit = (
      field: ConfigField,
      propertyPath: string[],
      underOptional: boolean,
      required: boolean,
    ): Effect.Effect<void, UnsupportedSchemaError> =>
      Effect.gen(function* () {
        const path = yield* Effect.try({
          try: () => {
            const path = propertyPath.map(configName)
            const key = path.join("/")
            if (paths.has(key))
              throw new Error(`duplicate canonical config path '${path.join(".")}'`)
            paths.add(key)
            return path
          },
          catch: (cause) => new UnsupportedSchemaError(`${contextLabel}: ${String(cause)}`),
        })
        if (!Schema.isSchema(field)) {
          yield* Effect.try({
            try: () => validateStaticSchema(field.schema.ast, `${contextLabel}: ${path.join(".")}`),
            catch: (cause) =>
              cause instanceof UnsupportedSchemaError
                ? cause
                : new UnsupportedSchemaError(`${contextLabel}: cannot inspect static schema`),
          })
          const nativeCodec = (yield* compile(field.schema)) as CompiledWitCodec<Schema.Top>
          const graph = yield* Effect.try({
            try: () => configGraph(nativeCodec.graph),
            catch: (cause) => new UnsupportedSchemaError(`${contextLabel}: ${String(cause)}`),
          })
          const codec = { ...nativeCodec, graph, schemaGraph: schemaGraphToWit(graph) }
          const ref = new SchemaRef(codec.schemaGraph)
          const eligible = ref.jsonEligibility()
          if (!eligible.success) {
            return yield* Effect.fail(
              new UnsupportedSchemaError(
                `${contextLabel}: static config '${path.join(".")}' must contain data only: ${eligible.issues[0]?.message}`,
              ),
            )
          }
          const defaultValue =
            "defaultValue" in field
              ? yield* Effect.try({
                  try: () =>
                    Effect.runSync(
                      codec.encode(field.defaultValue) as Effect.Effect<
                        CoreTypes.SchemaValueTree,
                        Schema.SchemaError
                      >,
                    ),
                  catch: () =>
                    new UnsupportedSchemaError(
                      `${contextLabel}: cannot export static code default at '${path.join(".")}'; use a schema-valid data value with no services or callbacks`,
                    ),
                })
              : undefined
          leaves.push({
            source: "local",
            path,
            propertyPath,
            schema: field.schema,
            codec,
            declarationGraph: codec.graph,
            required: true,
            static: true,
            defaultValue,
          })
          return
        }
        const schema = field as Schema.Top
        const inner = redactedInner(schema)
        if (inner !== undefined) {
          const valueSchema = underOptional ? Schema.UndefinedOr(inner) : inner
          const codec = (yield* compile(valueSchema)) as CompiledWitCodec<Schema.Top>
          const declarationGraph = { defs: codec.graph.defs, root: t.secret(codec.graph.root) }
          leaves.push({
            source: "secret",
            path,
            propertyPath,
            schema: valueSchema,
            codec,
            declarationGraph,
            required: true,
          })
          return
        }
        const object = objectAst(schema.ast)
        if (object !== undefined) {
          branches.add(propertyPath.join("/"))
          for (const property of object.ast.propertySignatures) {
            if (typeof property.name !== "string") {
              return yield* Effect.fail(
                new UnsupportedSchemaError(`${contextLabel}: config object keys must be strings`),
              )
            }
            const optional = property.type.context?.isOptional === true
            yield* visit(
              Schema.make(property.type) as Schema.Top,
              [...propertyPath, property.name],
              underOptional || object.optional,
              !optional,
            )
          }
          return
        }
        let codec = (yield* compile(schema)) as CompiledWitCodec<Schema.Top>
        let valueSchema = schema
        if (underOptional && codec.graph.root.body.tag !== "option") {
          const optionalSchema = Schema.UndefinedOr(schema)
          valueSchema = optionalSchema
          codec = (yield* compile(optionalSchema)) as unknown as CompiledWitCodec<Schema.Top>
        }
        leaves.push({
          source: "local",
          path,
          propertyPath,
          schema: valueSchema,
          codec,
          declarationGraph: codec.graph,
          required,
        })
      })
    for (const [name, schema] of Object.entries(fields)) yield* visit(schema, [name], false, true)
    const leavesByPath = new Map(leaves.map((leaf) => [leaf.propertyPath.join("/"), leaf]))
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
          defaultValue: leaf.defaultValue,
        }))
      },
      buildShape: compiledConfigRuntime(
        leaves.map((leaf) => ({
          ...leaf,
          declarationSchema: schemaGraphToWit(leaf.declarationGraph),
        })),
        branches,
      ).buildShape,
    }
  })

export interface ConfigStatics<F extends ConfigFields> {
  readonly fields: F
  /** Construct the service in the current owner context. @since 1.6.0 @category layers */
  readonly layer: Layer.Layer<ConfigShape<F>, UnsupportedSchemaError, ConfigClient | SecretsClient>
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
    static readonly layer = Layer.effect(
      Base,
      Effect.flatMap(
        Effect.suspend(() => Base.__compile()),
        (compiled) =>
          compiled.buildShape() as Effect.Effect<
            ConfigShape<F>,
            never,
            ConfigClient | SecretsClient
          >,
      ),
    )
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
  compiled: {
    readonly branches: ReadonlySet<string>
    readonly leavesByPath: ReadonlyMap<string, Pick<WireConfigLeaf, "path" | "source" | "codec">>
  },
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
          path: [...leaf.path],
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
