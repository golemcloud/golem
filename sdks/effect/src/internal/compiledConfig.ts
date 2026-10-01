import { Context, Effect, Redacted, Schema } from "effect"
import type * as Core from "golem:core/types@2.0.0"
import type * as Common from "golem:agent/common@2.0.0"
import { ConfigError } from "../Config.js"
import { ConfigClient } from "../host/ConfigClient.js"
import { SecretsClient } from "../host/SecretsClient.js"

export interface WireConfigLeaf {
  readonly source: Common.AgentConfigSource
  readonly path: ReadonlyArray<string>
  readonly codec: {
    readonly schemaGraph: Core.SchemaGraph
    readonly encode: (
      value: unknown,
    ) => Effect.Effect<Core.SchemaValueTree, Schema.SchemaError, any>
    readonly decode: (tree: Core.SchemaValueTree) => Effect.Effect<unknown, Schema.SchemaError, any>
  }
  readonly declarationSchema: Core.SchemaGraph
}

export function compiledConfigRuntime(
  leaves: ReadonlyArray<WireConfigLeaf>,
  branches: ReadonlySet<string>,
) {
  const ensure = (root: Record<string, unknown>, path: ReadonlyArray<string>) => {
    let cursor = root
    for (const segment of path) cursor = (cursor[segment] ??= {}) as Record<string, unknown>
    return cursor
  }
  return {
    leavesByPath: new Map(leaves.map((leaf) => [leaf.path.join("/"), leaf])),
    branches,
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
              try: () => config.getConfigValue(leaf.path, leaf.declarationSchema),
              catch: (cause) => new ConfigError(leaf.path, { _tag: "HostTrap", cause }),
            })
            return yield* Effect.mapError(
              leaf.codec.decode(tree),
              (cause) => new ConfigError(leaf.path, { _tag: "DecodeFailure", cause }),
            )
          })
          const borrow = Effect.gen(function* () {
            const tree = yield* Effect.try({
              try: () => config.getConfigValue(leaf.path, leaf.declarationSchema),
              catch: (cause) => new ConfigError(leaf.path, { _tag: "HostTrap", cause }),
            })
            const node = tree.valueNodes[tree.root]
            if (node?.tag !== "secret-value")
              return yield* Effect.fail(
                new ConfigError(leaf.path, {
                  _tag: "Unsupported",
                  reason: "expected secret handle",
                }),
              )
            return node.val
          })
          const value =
            leaf.source === "secret"
              ? {
                  borrow,
                  get: Effect.gen(function* () {
                    const raw = yield* borrow
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
}

export function compiledConfigService(
  name: string,
  fields: unknown,
  compile: (fields: unknown) => ReturnType<typeof compiledConfigRuntime>,
): Context.ServiceClass<never, string, unknown> & {
  readonly fields: unknown
  readonly __wireConfig: ReturnType<typeof compiledConfigRuntime>
} {
  return class extends Context.Service<never, unknown>()(name) {
    static readonly fields = fields
    static readonly __wireConfig = compile(fields)
  }
}
