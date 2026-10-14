import { Effect, Schema } from "effect"
import { defineAgent } from "@golemcloud/effect-golem/Agent"
import { method } from "@golemcloud/effect-golem/Method"

defineAgent({
  name: "SubpathAgent",
  id: { initial: Schema.Number },
  methods: { get: method({ input: {}, success: Schema.Number }) },
}).implement({
  init: ({ initial }) => Effect.succeed({ value: initial }),
  methods: (state) => ({ get: () => Effect.succeed(state.value) }),
})
