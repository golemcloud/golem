import { Effect, Schema } from "effect"
import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem"

interface CounterState {
  readonly value: number
}

defineAgent({
  name: "CapabilityCounter",
  id: { initial: Schema.Number },
  snapshotting: Snapshot.define({
    schema: Schema.Struct({ value: Schema.Number }),
    policy: Snapshot.policy.everyN(5),
  }),
  methods: { get: method({ input: {}, success: Schema.Number }) },
}).implement<CounterState>({
  init: ({ initial }): Effect.Effect<CounterState> => Effect.succeed({ value: initial }),
  methods: (state) => ({ get: () => Effect.succeed(state.value) }),
})
