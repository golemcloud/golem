import { Effect, Schema } from "effect"
import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem"
import { AgentConfig } from "./runtime-config.js"

const FilePath = Schema.String.check(Schema.isPattern(/^(?:workspace|revisions)\/.+$/))
const SnapshotState = Schema.Struct({ path: FilePath })

function metadata() {
  ;(globalThis as any).__metadataEvaluated?.()
  return {
    name: ["Runtime", "Metadata"].join(""),
    id: { path: Schema.String },
    config: AgentConfig,
    snapshotting: Snapshot.define({ schema: SnapshotState, policy: Snapshot.policy.everyN(10) }),
    methods: {
      path: method({ input: {}, success: FilePath }),
      configured: method({
        input: {},
        success: Schema.Struct({ apiUrl: Schema.String, hasSecret: Schema.Boolean }),
      }),
    },
  }
}

defineAgent(metadata()).implement<{ readonly path: string }>({
  init: ({ path }) => Effect.succeed({ path }),
  methods: (state) => ({
    path: () => Effect.succeed(state.path),
    configured: () =>
      Effect.gen(function* () {
        const config = yield* AgentConfig
        return {
          apiUrl: yield* config.apiUrl,
          hasSecret: (yield* config.apiKey.borrow) !== undefined,
        }
      }),
  }),
})
