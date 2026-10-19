import { Effect, Schema } from "effect"
import { defineAgent, Http, method, Method } from "../src/index.js"
import { Principal, PrincipalSchema } from "../src/Principal.js"

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
    explicitPrincipal: method({
      input: { principal: PrincipalSchema },
      success: Schema.String,
      error: Schema.String,
      readOnly: { cache: { ttlNanos: 1n } },
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    // @ts-expect-error Principal is unavailable to read-only handlers
    literal: () => Effect.as(Principal, "literal"),
    // @ts-expect-error Principal is unavailable for read-only cache options
    options: () => Effect.as(Principal, "options"),
    // @ts-expect-error withHttp must preserve the read-only handler services
    withHttp: () => Effect.as(Principal, "http"),
    ordinary: ({ value }) => Effect.as(Principal, String(value)),
    explicitPrincipal: ({ principal }) =>
      principal.tag === "anonymous" ? Effect.fail("anonymous") : Effect.succeed(principal.tag),
  }),
})
