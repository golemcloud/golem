import { Effect, Schema } from "effect"
import { defineAgent, Http, method, Method } from "../src/index.js"
import { InvocationContext } from "../src/InvocationContext.js"

defineAgent({
  name: "ReadOnlyHandlerServices",
  id: {},
  http: Http.mount("/read-only"),
  methods: {
    literal: method({ input: {}, success: Schema.String, readOnly: true }),
    options: method({
      input: {},
      success: Schema.String,
      readOnly: { cache: "no-cache" },
    }),
    withHttp: method({ input: {}, success: Schema.String, readOnly: true }).pipe(
      Method.withHttp(Http.get("/value")),
    ),
    ordinary: method({ input: { value: Schema.Number }, success: Schema.String }),
    ttl: method({
      input: {},
      success: Schema.String,
      error: Schema.String,
      readOnly: { cache: { ttlNanos: 1n } },
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    // Every handler, read-only ones included, may read the invocation context.
    literal: () => Effect.map(InvocationContext, (context) => context.caller.immediate.kind),
    options: () => Effect.map(InvocationContext, (context) => context.caller.immediate.kind),
    withHttp: () => Effect.map(InvocationContext, (context) => context.caller.immediate.kind),
    ordinary: ({ value }) => Effect.as(InvocationContext, String(value)),
    ttl: () =>
      Effect.flatMap(InvocationContext, (context) =>
        context.caller.immediate.kind === "anonymous"
          ? Effect.fail("anonymous")
          : Effect.succeed(context.caller.immediate.kind),
      ),
  }),
})
