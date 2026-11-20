import { Effect, Schema } from "effect"
import { defineAgent, method } from "@golemcloud/effect-golem"

const value = [3, 5, 7].reduce((sum, item) => sum * 2 + item, 0)

defineAgent({
  name: "JsGuestSmoke",
  description: `Effect initialized value: ${value}, clock: ${Date.now()}`,
  id: {},
  methods: { get: method({ input: {}, success: Schema.Number }) },
}).implement({
  init: () => Effect.succeed({}),
  methods: () => ({ get: () => Effect.succeed(value) }),
})
