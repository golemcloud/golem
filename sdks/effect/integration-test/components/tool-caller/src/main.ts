import { Effect, Schema, Stream } from "effect"
import { defineAgent, method, Middleware, Tool } from "@golemcloud/effect-golem"

const definition = Tool.toolDefinition("effect-streaming").body((body) =>
  body
    .positional("message", Schema.String)
    .input({ required: true })
    .output({ required: true })
    .returns(Schema.String),
)

Middleware.typed({
  name: "effect-combined-prefix",
  presented: definition,
  handler: {
    effectStreaming: ({ message }, { stdin, stdout, underlying }) =>
      underlying(
        { message: `middleware:${message}` },
        {
          stdin,
          stdout: (stream) => stdout!(stream),
        },
      ),
  },
})

const client = Tool.client(definition)

defineAgent({
  name: "EffectToolCaller",
  id: { name: Schema.String },
  methods: {
    roundtrip: method({ input: { payload: Schema.String }, success: Schema.String }),
  },
}).implement(({ name }) =>
  Effect.succeed({
    roundtrip: ({ payload }) =>
      Effect.gen(function* () {
        let stdout = ""
        const result = yield* client(
          { message: name },
          {
            stdin: Stream.succeed(new TextEncoder().encode(payload)),
            stdout: (stream) =>
              stream.pipe(
                Stream.decodeText(),
                Stream.runForEach((chunk) =>
                  Effect.sync(() => {
                    stdout += chunk
                  }),
                ),
              ),
          },
        )
        return `${result}|${stdout}`
      }).pipe(Effect.orDie),
  }),
)
