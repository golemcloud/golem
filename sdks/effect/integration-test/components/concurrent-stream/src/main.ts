import { Effect, Schema, Stream } from "effect"
import { Socket } from "effect/socket"
import { defineAgent, Http, method, Websocket, WitTypes } from "@golemcloud/effect-golem"

defineAgent({
  name: "ConcurrentStreamProbe",
  id: { name: Schema.String, readers: Schema.Int },
  http: Http.mount("/stream-probe/{name}/{readers}"),
  methods: {
    echo: method({
      input: { input: WitTypes.AgentStream(Schema.String) },
      success: WitTypes.AgentStream(Schema.String),
      http: [
        Http.put("/echo", {
          durableStreams: {
            slots: [
              { source: "input", slot: "input" },
              { source: "output", slot: "$result" },
            ],
            allowExternalWrites: true,
          },
        }),
      ],
    }),
  },
}).implement({
  init: ({ readers }) => Effect.succeed(readers),
  methods: (readers) => ({
    echo: ({ input }) =>
      Effect.gen(function* () {
        const stream = Stream.unwrap(
          Effect.gen(function* () {
            if (readers === 0) return input
            const first = yield* Websocket.connect("ws://127.0.0.1:19110/first")
            const firstPull = yield* Socket.readerString(first)
            const write = yield* first.writer
            const secondPull =
              readers === 2
                ? yield* Websocket.connect("ws://127.0.0.1:19110/second").pipe(
                    Effect.flatMap(Socket.readerString),
                  )
                : undefined
            yield* firstPull.pipe(Effect.forever, Effect.forkScoped)
            if (secondPull) yield* secondPull.pipe(Effect.forever, Effect.forkScoped)
            return input.pipe(
              Stream.mapEffect((text) =>
                Effect.gen(function* () {
                  yield* Effect.logInfo("Input handler entered")
                  yield* write(new TextEncoder().encode(text))
                  return text
                }),
              ),
            )
          }),
        )
        const context = yield* Effect.context<Stream.Services<typeof stream>>()
        return stream.pipe(Stream.provideContext(context))
      }),
  }),
})
