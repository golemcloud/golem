import { Effect, HashMap, Option, Schema, Stream } from "effect"
import {
  Principal,
  Multimodal,
  Tool,
  Unstructured,
  WitTypes,
  defineAgent,
  defineConfig,
  method,
} from "@golemcloud/effect-golem"
import { ForeignStream, Remote } from "./remote-definition.mjs"

const remoteToolDefinition = Tool.toolDefinition("remote-tool").command("do-work", (command) =>
  command.body((body) =>
    body
      .positional("some-number", Schema.NumberFromString)
      .returns(Schema.NumberFromString)
      .error("denied", Schema.String),
  ),
)
const toolRemote = Tool.client(remoteToolDefinition, {
  transport: { start: (...args) => globalThis.__concreteToolStart(...args) },
})

const Choice = Schema.Union([
  Schema.Struct({ _tag: Schema.Literal("text"), text: Schema.String }),
  Schema.Struct({ _tag: Schema.Literal("numbers"), values: Schema.Array(Schema.Number) }),
])
const Payload = Schema.Struct({ choice: Choice, optional: Schema.Option(Schema.NumberFromString) })
const OptionalFields = Schema.Struct({
  text: Schema.optional(Schema.String),
  count: Schema.optionalKey(Schema.NumberFromString),
  option: Schema.optionalKey(Schema.Option(Schema.Number)),
})
const Rich = Schema.Struct({
  words: WitTypes.FixedList(Schema.String, 2),
  numbers: WitTypes.Uint16ArraySchema,
  names: WitTypes.Map(Schema.String, Schema.NumberFromString),
  lookup: Schema.HashMap(Schema.String, Schema.Number),
  text: WitTypes.Text(),
  binary: WitTypes.Binary(),
  flags: WitTypes.Flags(["read", "write", "admin"]),
  elapsed: WitTypes.Duration,
  who: Principal.PrincipalSchema,
})
const Settings = defineConfig("ConcreteSettings", {
  nested: Schema.Struct({ number: Schema.NumberFromString }),
  token: Schema.Redacted(Schema.String),
})

Tool.toolDefinition("concrete")
  .body((body) =>
    body.positional("value", Payload).returns(Payload).error("rejected", Schema.String),
  )
  .implement({
    concrete: ({ value }) =>
      value.choice._tag === "text" && value.choice.text === "reject"
        ? Effect.fail(Tool.err("rejected", "blocked"))
        : Effect.succeed({ ...value, optional: Option.some(73) }),
  })

defineAgent({
  name: "ConcreteAgent",
  id: {},
  config: Settings,
  methods: {
    echo: method({ input: { value: Payload }, success: Payload, error: Schema.String }),
    rich: method({ input: { value: Rich }, success: Rich }),
    optionalFields: method({ input: { value: OptionalFields }, success: OptionalFields }),
    tool: method({ input: {}, success: Schema.Number, error: Schema.String }),
    text: method({
      input: { value: Unstructured.UnstructuredText({ restrictions: [{ languageCode: "en" }] }) },
      success: Unstructured.UnstructuredText({ restrictions: [{ languageCode: "en" }] }),
    }),
    content: method({
      input: {
        value: Multimodal.multimodal({
          text: Unstructured.UnstructuredText({ restrictions: [{ languageCode: "en" }] }),
          binary: Unstructured.UnstructuredBinary({ restrictions: [{ mimeType: "image/png" }] }),
          count: Schema.NumberFromString,
        }),
      },
      success: Schema.Array(Schema.String),
    }),
    remote: method({
      input: { value: Schema.Number },
      success: Schema.Number,
      error: Schema.String,
    }),
    collect: method({
      input: { values: ForeignStream },
      success: Schema.Array(Schema.Number),
    }),
    configured: method({
      input: {},
      success: Schema.Struct({ number: Schema.Number, fresh: Schema.Boolean }),
    }),
  },
}).implement({
  init: () => Effect.succeed(undefined),
  methods: () => ({
    tool: () =>
      toolRemote
        .doWork({ someNumber: 23 })
        .pipe(
          Effect.mapError((e) => (e._tag === "ToolFailure" ? `${e.name}:${e.value}` : e.phase)),
        ),
    echo: ({ value }) =>
      value.choice._tag === "text" ? Effect.fail(value.choice.text) : Effect.succeed(value),
    optionalFields: ({ value }) => Effect.succeed(value),
    text: ({ value }) => Effect.succeed(value),
    content: ({ value }) =>
      Effect.succeed(
        value.map((item) =>
          item._tag === "count"
            ? `count:${item.value + 1}`
            : `${item._tag}:${item.value.val.length}`,
        ),
      ),
    remote: ({ value }) =>
      Remote.client.get({ seed: 3 }).pipe(
        Effect.flatMap((remote) => remote.echo({ value })),
        Effect.scoped,
      ),
    rich: ({ value }) => {
      if (
        !(value.numbers instanceof Uint16Array) ||
        !(value.names instanceof Map) ||
        !HashMap.isHashMap(value.lookup)
      )
        return Effect.die("lost concrete carrier")
      return Effect.succeed(value)
    },
    collect: ({ values }) => Stream.runCollect(values),
    configured: () =>
      Effect.gen(function* () {
        const config = yield* Settings
        const number = yield* config.nested.number
        const again = yield* config.nested.number
        const first = yield* config.token.borrow
        const second = yield* config.token.borrow
        return { number: number + again, fresh: first !== second }
      }),
  }),
})
