import { Effect, Schema as ES } from "effect"
import { defineAgent as agent, method, WitTypes as Wire } from "@golemcloud/effect-golem"

export const ForeignStream = Wire.AgentStream(ES.NumberFromString)
export const Remote = agent({
  name: "Remote",
  id: { seed: ES.NumberFromString },
  methods: {
    echo: method({
      input: { value: ES.NumberFromString },
      success: ES.NumberFromString,
      error: ES.String,
    }),
  },
}).implement({
  init: ({ seed }) => Effect.succeed(seed),
  methods: (seed) => ({ echo: ({ value }) => Effect.succeed(seed + value) }),
})
