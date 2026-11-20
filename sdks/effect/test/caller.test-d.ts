import { Effect, Schema } from "effect"
import { defineAgent, InvocationContext, method, Method } from "../src/index.js"

// A PrincipalRef is plain data in every position.
method({
  input: { who: InvocationContext.PrincipalRefSchema },
  success: InvocationContext.PrincipalRefSchema,
})
method({
  input: { data: Schema.Struct({ who: InvocationContext.PrincipalRefSchema }) },
  success: Schema.Array(InvocationContext.PrincipalRefSchema),
})
defineAgent({
  name: "Ok",
  id: { owner: InvocationContext.PrincipalRefSchema },
  methods: {},
})

// A handler reads the caller from the service, read-only handlers included.
defineAgent({
  name: "Handler",
  id: {},
  methods: {
    run: method({ input: { n: Schema.Number }, success: Schema.String }),
    peek: method({ input: {}, success: Schema.String, readOnly: true }),
  },
}).implement({
  init: () =>
    Effect.gen(function* () {
      const context = yield* InvocationContext.InvocationContext
      const caller: InvocationContext.Caller = context.caller
      void caller
    }),
  methods: () => ({
    run: ({ n }) =>
      Effect.map(InvocationContext.InvocationContext, (context) => {
        const principal: InvocationContext.Principal = context.caller.original
        return `${n}:${principal.kind}`
      }),
    peek: () =>
      Effect.map(InvocationContext.InvocationContext, (context) => context.caller.immediate.kind),
  }),
})

declare const live: Schema.Codec<InvocationContext.Principal>
declare const liveCaller: Schema.Codec<InvocationContext.Caller>
declare const liveToken: Schema.Codec<InvocationContext.TokenProvenance>

// @ts-expect-error a Principal cannot be a method input
method({ input: { who: live }, success: Schema.Void })
// @ts-expect-error a Caller cannot be a method input
method({ input: { who: liveCaller }, success: Schema.Void })
// @ts-expect-error a TokenProvenance cannot be a method input
method({ input: { token: liveToken }, success: Schema.Void })
// @ts-expect-error a Principal cannot be inside an input object
method({ input: { data: Schema.Struct({ who: live }) }, success: Schema.Void })
// @ts-expect-error a Principal cannot be inside an input array
method({ input: { data: Schema.Array(live) }, success: Schema.Void })
// @ts-expect-error a Principal cannot be an optional input
method({ input: { data: Schema.optional(live) }, success: Schema.Void })
// @ts-expect-error a Principal cannot be returned
method({ input: {}, success: live })
// @ts-expect-error a Caller cannot be returned inside an object
method({ input: {}, success: Schema.Struct({ caller: liveCaller }) })
// @ts-expect-error a Principal cannot be a typed error
method({ input: {}, success: Schema.Void, error: live })
// @ts-expect-error a Principal cannot be part of the agent id
defineAgent({ name: "Bad", id: { who: live }, methods: {} })
// @ts-expect-error a Principal cannot be an input of a defined method
Method.defineMethod({
  name: "bad",
  input: { who: live },
  success: Schema.Void,
  body: () => Effect.void,
})
