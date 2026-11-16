import { readFileSync } from "node:fs"
import { beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Layer, Schema } from "effect"
import * as Config from "../src/Config.js"
import { ConfigClient } from "../src/host/ConfigClient.js"
import { SecretsLive } from "../src/host/SecretsClient.js"
import { ConfigOwner } from "../src/internal/compiledConfig.js"
import { guest, loadSnapshot, saveSnapshot } from "../src/internal/guest.js"
import { __resetAgents, defineAgent } from "../src/Agent.js"
import { method } from "../src/Method.js"
import { SchemaRef, type JsonValue } from "../src/SchemaRef.js"
import {
  emptyMetadata,
  field,
  schemaGraphsEquivalent,
  schemaType,
  t,
  v,
  type SchemaType,
} from "../src/internal/schema-model/model.js"
import {
  schemaGraphFromWit,
  schemaValueFromWit,
  schemaValueToWit,
} from "../src/internal/schema-model/wit.js"
import { DiscriminatedUnion, Int32, Uint32 } from "../src/WitTypes.js"
import { UnsupportedSchemaError } from "../src/WitCodec.js"
import { StaticDefaultsConfig } from "../integration-test/components/static-config/src/main.js"
import {
  __resetGetConfigValueImpl,
  __setGetConfigValueImpl,
  __setParseAgentIdImpl,
} from "./mocks/golem-agent-host.js"
import { __setEnvironment } from "./mocks/wasi-cli-environment.js"

const corpus = JSON.parse(
  readFileSync(new URL("../../../test-data/config-conformance/v1.json", import.meta.url), "utf8"),
)
const principal = { tag: "anonymous" } as const
const empty = () => schemaValueToWit(v.record([]))

// Translate only the native schema kinds used by this hand-authored corpus.
const corpusType = (node: any): SchemaType => {
  const value = node.value
  switch (node.kind) {
    case "ref":
      return t.ref(value.id)
    case "record":
      return t.record(value.fields.map((entry: any) => field(entry.name, corpusType(entry.body))))
    case "tuple":
      return t.tuple(value.elements.map(corpusType))
    case "option":
      return t.option(corpusType(value.inner))
    case "list":
      return t.list(corpusType(value.element))
    case "union":
      return t.union(
        value.spec.branches.map((branch: any) => ({
          tag: branch.tag,
          body: corpusType(branch.body),
          metadata: emptyMetadata(),
          discriminator: {
            tag: branch.discriminator.rule,
            val:
              branch.discriminator.rule === "prefix"
                ? branch.discriminator.value.prefix
                : branch.discriminator.value,
          },
        })),
      )
    case "string":
    case "bool":
    case "u32":
    case "s32":
      return schemaType({ tag: node.kind })
    default:
      throw new Error(`Unmapped corpus kind: ${node.kind}`)
  }
}

interface Node {
  readonly displayName: string
  readonly next: Node | null
}
const Node: Schema.Codec<Node> = Schema.suspend(() =>
  Schema.Struct({
    displayName: Schema.String,
    next: Schema.NullOr(Node),
  }),
)

const schemaFields: Record<string, Config.ConfigFields> = {
  reference: StaticDefaultsConfig.fields,
  recursive: {
    tree: Config.static(Node, {
      defaultValue: { displayName: "outer", next: { displayName: "inner", next: null } },
    }),
  },
  canonicalName: { maxChars: Config.static(Uint32, { defaultValue: 19 }) },
  optionalDefault: {
    optional: Config.static(Schema.NullOr(Schema.String), { defaultValue: "from-code" }),
  },
  emptyValues: {
    enabled: Config.static(Schema.Boolean, { defaultValue: true }),
    names: Config.static(Schema.Array(Schema.String), { defaultValue: ["from-code"] }),
    label: Config.static(Schema.String, { defaultValue: "from-code" }),
  },
  recordUnion: {
    delivery: Config.static(
      DiscriminatedUnion([
        {
          tag: "mail",
          schema: Schema.Struct({
            kind: Schema.String,
            address: Schema.String,
            attempts: Uint32,
          }),
          discriminator: { tag: "field-equals", val: { fieldName: "kind", literal: "mail" } },
        },
        {
          tag: "queue",
          schema: Schema.Struct({ kind: Schema.String, queueName: Schema.String }),
          discriminator: { tag: "field-equals", val: { fieldName: "kind", literal: "queue" } },
        },
      ]),
      { defaultValue: { kind: "mail", address: "ops@example.test", attempts: 3 } },
    ),
  },
}

describe("portable static config", () => {
  beforeEach(async () => {
    await __resetAgents()
    __resetGetConfigValueImpl()
  })

  it.effect("exports schema-equivalent declarations and defaults from the independent corpus", () =>
    Effect.gen(function* () {
      for (const [name, fields] of Object.entries(schemaFields)) {
        const compiled = yield* Config.compileConfig(fields, name)
        const expected = corpus.schemas[name]
        expect(compiled.leaves.map((leaf) => leaf.path)).toEqual(
          expected.declarations.map((declaration: any) => declaration.path),
        )
        for (const [index, leaf] of compiled.leaves.entries()) {
          const declaration = expected.declarations[index]
          const graph = {
            defs: new Map<string, { body: SchemaType }>(
              (expected.graph.defs ?? []).map((definition: any) => [
                definition.id,
                { body: corpusType(definition.body) },
              ]),
            ),
            root: corpusType(declaration.value_type),
          }
          expect(
            schemaGraphsEquivalent(leaf.declarationGraph, graph),
            `${name}/${leaf.path.join("/")}`,
          ).toBe(true)
          expect(leaf.source).toBe("local")
          expect(
            leaf.defaultValue === undefined ? undefined : JSON.parse(leaf.defaultValue),
          ).toEqual(
            declaration.default_value === undefined
              ? undefined
              : JSON.parse(declaration.default_value),
          )
        }
      }
      const compiled = yield* Config.compileConfig(
        {
          absent: Config.static(Schema.NullOr(Schema.String)),
          explicit: Config.static(Schema.NullOr(Schema.String), { defaultValue: null }),
        },
        "null",
      )
      expect(compiled.declarations([0, 1]).map((declaration) => declaration.defaultValue)).toEqual([
        undefined,
        "null",
      ])
      const agent = guest
        .discoverAgentTypes()
        .find((agent) => agent.typeName === "EffectStaticDefaultsAgent")!
      const reference = yield* StaticDefaultsConfig.__compile()
      expect(agent.config.map((declaration) => declaration.defaultValue)).toEqual(
        corpus.schemas.reference.declarations.map((declaration: any) => declaration.default_value),
      )
      for (const [index, declaration] of agent.config.entries()) {
        expect(declaration.path).toEqual(reference.leaves[index]!.path)
        expect(
          schemaGraphsEquivalent(
            schemaGraphFromWit({ ...agent.schema, root: declaration.valueType }),
            reference.leaves[index]!.declarationGraph,
          ),
        ).toBe(true)
      }
    }),
  )

  it.effect(
    "decodes every successful static corpus result without resolving or remerging it in the SDK",
    () =>
      Effect.gen(function* () {
        for (const test of corpus.staticCases) {
          if (test.expected === undefined || schemaFields[test.schema] === undefined) continue
          const compiled = yield* Config.compileConfig(schemaFields[test.schema]!, test.id)
          for (const leaf of compiled.leaves) {
            const expected = test.expected[leaf.path[0]!]
            const ref = new SchemaRef(leaf.codec.schemaGraph)
            const tree = ref.packJson(expected)
            const value = yield* leaf.codec.decode(tree) as Effect.Effect<
              unknown,
              Schema.SchemaError
            >
            expect(
              ref.unpackJson(
                yield* leaf.codec.encode(value) as Effect.Effect<any, Schema.SchemaError>,
              ),
              test.id,
            ).toEqual(expected)
          }
        }
      }),
  )

  it.effect("projects config names without changing native values or method schemas", () =>
    Effect.gen(function* () {
      const Delivery = DiscriminatedUnion([
        {
          tag: "queue",
          schema: Schema.Struct({ delivery_kind: Schema.String, queue_name: Schema.String }),
          discriminator: {
            tag: "field-equals",
            val: { fieldName: "delivery_kind", literal: "queue" },
          },
        },
      ])
      const native = { delivery_kind: "queue", queue_name: "limits.support_default" }
      interface NativeNode {
        readonly display_name: string
        readonly next: NativeNode | null
      }
      const Recursive: Schema.Codec<NativeNode> = Schema.suspend(() =>
        Schema.Struct({ display_name: Schema.String, next: Schema.NullOr(Recursive) }),
      )
      const tree = { display_name: "outer", next: { display_name: "inner", next: null } }
      const Settings = Config.defineConfig("Native.Names", {
        max_chars: Config.static(Uint32, { defaultValue: 19 }),
        delivery: Config.static(Delivery, { defaultValue: native }),
        tree: Config.static(Recursive, { defaultValue: tree }),
      })
      defineAgent({
        name: "NativeNamesAgent",
        id: {},
        config: Settings,
        methods: {
          echo: method({ input: { value: Delivery }, success: Delivery }),
          echoTree: method({ input: { value: Recursive }, success: Recursive }),
        },
      }).implement({
        init: () => Effect.void,
        methods: () => ({
          echo: ({ value }) => Effect.succeed(value),
          echoTree: ({ value }) => Effect.succeed(value),
        }),
      })
      const compiled = yield* Settings.__compile()
      expect(compiled.leaves.map((leaf) => leaf.path)).toEqual([
        ["maxChars"],
        ["delivery"],
        ["tree"],
      ])
      expect(JSON.parse(compiled.leaves[1]!.defaultValue!)).toEqual({
        deliveryKind: "queue",
        queueName: "limits.support_default",
      })
      const entries = yield* Config.encodeOverrides(compiled, {
        max_chars: 0,
        delivery: native,
        tree,
      })
      expect(entries.map((entry) => entry.path)).toEqual([["maxChars"], ["delivery"], ["tree"]])
      const owner = {}
      const shape = yield* compiled.buildShape().pipe(
        Effect.provideService(ConfigOwner, owner),
        Effect.provide(
          Layer.succeed(
            ConfigClient,
            ConfigClient.of({
              getConfigValue: (path) => {
                expect(path[0]).toBe("maxChars")
                return entries[0]!.value.value
              },
            }),
          ),
        ),
      ) as Effect.Effect<{ max_chars: Effect.Effect<number, Config.ConfigError> }>
      expect(yield* shape.max_chars.pipe(Effect.provideService(ConfigOwner, owner))).toBe(0)
      const metadata = guest
        .discoverAgentTypes()
        .find((agent) => agent.typeName === "NativeNamesAgent")!
      const declaration = metadata.config.find((field) => field.path[0] === "delivery")!
      const configRef = new SchemaRef({ ...metadata.schema, root: declaration.valueType })
      expect(configRef.unpackJson(entries[1]!.value.value)).toEqual({
        deliveryKind: "queue",
        queueName: "limits.support_default",
      })
      const output = metadata.methods.find((method) => method.name === "echo")!.outputSchema
      if (output.tag !== "single") throw new Error("expected an echo result schema")
      expect(
        new SchemaRef({ ...metadata.schema, root: output.val }).unpackJson(entries[1]!.value.value),
      ).toEqual(native)
      const treeDeclaration = metadata.config.find((field) => field.path[0] === "tree")!
      expect(
        new SchemaRef(metadata.schema, treeDeclaration.valueType).unpackJson(
          entries[2]!.value.value,
        ),
      ).toEqual({
        displayName: "outer",
        next: { displayName: "inner", next: null },
      })
      const treeOutput = metadata.methods.find((method) => method.name === "echoTree")!.outputSchema
      if (treeOutput.tag !== "single") throw new Error("expected a recursive result schema")
      expect(
        new SchemaRef(metadata.schema, treeOutput.val).unpackJson(entries[2]!.value.value),
      ).toEqual(tree)
      const result = yield* Effect.result(
        Config.compileConfig(
          {
            max_chars: Config.static(Uint32),
            maxChars: Config.static(Uint32),
          },
          "collision",
        ),
      )
      expect(result._tag).toBe("Failure")
    }),
  )

  it.effect(
    "encodes whole-value creation overrides and preserves positional absence and exact names",
    () =>
      Effect.gen(function* () {
        const compiled = yield* StaticDefaultsConfig.__compile()
        const overrides = { settings: { label: "creation", count: 59 }, slots: [11, null, "third"] }
        const entries = yield* Config.encodeOverrides(compiled, overrides)
        expect(entries.map((entry) => entry.path)).toEqual([["settings"], ["slots"]])
        expect(
          entries.map((entry) => new SchemaRef(entry.value.graph).unpackJson(entry.value.value)),
        ).toEqual(Object.values(overrides))
        expect(
          (yield* Effect.result(Config.encodeOverrides(compiled, { Settings: overrides.settings })))
            ._tag,
        ).toBe("Failure")
      }),
  )

  it.effect(
    "rejects invalid data defaults, capabilities, reserved names and nonportable schema factories",
    () =>
      Effect.gen(function* () {
        let calls = 0
        const factory = Schema.String.pipe(
          Schema.withConstructorDefault(
            Effect.sync(() => {
              calls++
              return "hidden"
            }),
          ),
        )
        const invalid: Config.ConfigFields[] = [
          { count: Config.static(Uint32, { defaultValue: -1 }) },
          { count: Config.static(Uint32, { defaultValue: (() => 1) as never }) },
          { $reserved: Config.static(Schema.String) },
          {
            value: Config.static(
              Schema.Struct({ nested: Schema.Struct({ $config: Schema.String }) }),
            ),
          },
          { value: Config.static(Schema.NullOr(Schema.Array(Schema.Redacted(Schema.String)))) },
          { value: Config.static(Schema.Struct({ nested: factory })) },
        ]
        for (const fields of invalid) {
          const result = yield* Effect.result(Config.compileConfig(fields, "invalid"))
          expect(result._tag).toBe("Failure")
          if (result._tag === "Failure")
            expect(result.failure).toBeInstanceOf(UnsupportedSchemaError)
        }
        expect(calls).toBe(0)
      }),
  )

  it.effect(
    "constructs explicit owner Layers, caches within an owner and rejects a retained service for another",
    () =>
      Effect.gen(function* () {
        const Settings = Config.defineConfig("Static.Layer", {
          maxChars: Config.static(Int32, { defaultValue: 19 }),
        })
        const compiled = yield* Settings.__compile()
        const owner = {}
        const other = {}
        let reads = 0
        let supplied = 0
        const host = Layer.succeed(
          ConfigClient,
          ConfigClient.of({
            getConfigValue: () => {
              reads++
              return Effect.runSync(
                compiled.leaves[0]!.codec.encode(supplied) as Effect.Effect<any, any>,
              )
            },
          }),
        )
        const layer = Settings.layer.pipe(Layer.provide(host), Layer.provide(SecretsLive))
        const get = Effect.gen(function* () {
          const settings = yield* Settings
          return yield* settings.maxChars
        }).pipe(Effect.provide(layer))
        expect(yield* get.pipe(Effect.provideService(ConfigOwner, owner))).toBe(0)
        supplied = 7
        expect(yield* get.pipe(Effect.provideService(ConfigOwner, owner))).toBe(0)
        expect(yield* get.pipe(Effect.provideService(ConfigOwner, other))).toBe(7)
        expect(reads).toBe(2)
        const retained = yield* Settings.pipe(
          Effect.provide(layer),
          Effect.provideService(ConfigOwner, owner),
        )
        const wrongOwner = yield* Effect.result(
          retained.maxChars.pipe(Effect.provideService(ConfigOwner, other)),
        )
        expect(wrongOwner._tag).toBe("Failure")
        expect((yield* Effect.result(get))._tag).toBe("Failure")
      }),
  )

  it.effect(
    "does not fall back to a code default when the host fails or supplies an invalid value",
    () =>
      Effect.gen(function* () {
        const Settings = Config.defineConfig("Static.NoFallback", {
          count: Config.static(Uint32, { defaultValue: 31 }),
        })
        for (const getConfigValue of [
          () => {
            throw new Error("broken supplied selector")
          },
          () => schemaValueToWit(v.string("invalid supplied value")),
        ]) {
          const read = Effect.gen(function* () {
            const settings = yield* Settings
            return yield* settings.count
          }).pipe(
            Effect.provide(
              Settings.layer.pipe(
                Layer.provide(Layer.succeed(ConfigClient, ConfigClient.of({ getConfigValue }))),
                Layer.provide(SecretsLive),
              ),
            ),
            Effect.provideService(ConfigOwner, {}),
          )
          const result = yield* Effect.result(read)
          expect(result._tag).toBe("Failure")
          if (result._tag === "Failure") expect(result.failure).toBeInstanceOf(Config.ConfigError)
        }
      }),
  )

  it("runs the real fixture's dispatcher with host-supplied corpus captures across owners", async () => {
    const compiled = await Effect.runPromise(StaticDefaultsConfig.__compile())
    let values: Record<string, JsonValue> = corpus.staticCases[0].expected
    __setGetConfigValueImpl((path) => {
      const leaf = compiled.leaves.find((leaf) => leaf.path.join("/") === path.join("/"))!
      return new SchemaRef(leaf.codec.schemaGraph).packJson(values[path[0]!]!)
    })
    const id = () => schemaValueToWit(v.record([v.string("owner")]))
    const read = async () => {
      const result = schemaValueFromWit((await guest.invoke("read", empty(), principal))!)
      if (result.tag !== "string") throw new Error("expected JSON string")
      return JSON.parse(result.value)
    }
    await guest.initialize("EffectStaticDefaultsAgent", id(), principal)
    expect(await read()).toEqual(corpus.staticCases[0].expected)
    const snapshot = await saveSnapshot.save()
    expect(new TextDecoder().decode(snapshot.payload)).not.toContain("code-default")
    await __resetAgents()
    __setEnvironment([["GOLEM_AGENT_ID", 'EffectStaticDefaultsAgent("owner")']])
    __setParseAgentIdImpl(() => ["EffectStaticDefaultsAgent", { value: id() }, undefined])
    await loadSnapshot.load(snapshot)
    expect(await read()).toEqual(corpus.staticCases[0].expected)
    values = corpus.staticCases[3].expected
    expect(await read()).toEqual(corpus.staticCases[0].expected)
    await __resetAgents()
    await guest.initialize("EffectStaticDefaultsAgent", id(), principal)
    expect(await read()).toEqual(values)
  })

  it("rejects a retained Layer through the public dispatcher after owner replacement", async () => {
    const Settings = Config.defineConfig("Static.Retained", { count: Config.static(Uint32) })
    let retained: typeof Settings.layer | undefined
    defineAgent({
      name: "RetainedStaticConfig",
      id: {},
      config: Settings,
      methods: { read: method({ input: {}, success: Uint32 }) },
    }).implement({
      init: () =>
        Effect.gen(function* () {
          retained ??= Layer.succeed(Settings, yield* Settings.pipe(Effect.provide(Settings.layer)))
          return yield* Settings.pipe(Effect.provide(retained))
        }),
      methods: (settings) => ({ read: () => settings.count }),
    })
    __setGetConfigValueImpl(() => schemaValueToWit(v.u32(11)))
    await guest.initialize("RetainedStaticConfig", empty(), principal)
    expect(schemaValueFromWit((await guest.invoke("read", empty(), principal))!)).toEqual(v.u32(11))
    await __resetAgents()
    await guest.initialize("RetainedStaticConfig", empty(), principal)
    await expect(guest.invoke("read", empty(), principal)).rejects.toBeDefined()
  })
})
