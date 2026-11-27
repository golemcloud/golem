import { Effect, Schema } from "effect"
import { parseUuid, uuidToString } from "golem:core/types@2.0.0"
import {
  Agents,
  Durability,
  SelfAgentId,
  Snapshot,
  defineAgent,
  method,
} from "@golemcloud/effect-golem"

for (const snapshots of [false, true]) {
  const spec = defineAgent({
    name: snapshots ? "SnapshotIdentity" : "Identity",
    id: { name: Schema.String },
    snapshotting: Snapshot.custom({
      policy: snapshots ? Snapshot.policy.everyN(2) : Snapshot.policy.manual,
    }),
    methods: {
      observe: method({ input: {}, success: Schema.String }),
      status: method({ input: {}, success: Schema.String }),
      forkAt: method({ input: { cutoff: Schema.String }, success: Schema.String }),
      forkSelf: method({ input: {}, success: Schema.String }),
    },
  })
  spec.implement({
    init: ({ name }) =>
      SelfAgentId.SelfAgentId.pipe(
        Effect.map((id) => ({
          configured: name,
          saved: id as string,
          trace: [] as string[],
          snapshotIdentity: "",
          lastFork: "",
        })),
      ),
    methods: (state) => ({
      observe: () =>
        Effect.gen(function* () {
          const self: string = yield* SelfAgentId.SelfAgentId
          // Different durable calls make an incorrect historical identity diverge.
          if (self === state.saved) yield* Agents.getSelfMetadata
          else yield* Durability.generateIdempotencyKey
          state.trace.push(self)
          return self
        }),
      status: () =>
        Effect.gen(function* () {
          const self = yield* SelfAgentId.SelfAgentId
          const metadata = yield* Agents.getSelfMetadata
          const addressed = yield* Agents.getAgentMetadata({ uuid: parseUuid(self) })
          return JSON.stringify({
            ...state,
            current: self,
            metadata: uuidToString(metadata.agentId.uuid),
            addressed: addressed ? uuidToString(addressed.agentId.uuid) : undefined,
          })
        }),
      forkAt: ({ cutoff }) =>
        Effect.gen(function* () {
          const metadata = yield* Agents.getSelfMetadata
          const forked = yield* Agents.forkAgent({
            source: metadata.agentId,
            oplogIdxCutOff: BigInt(cutoff),
          })
          return uuidToString(forked.uuid)
        }),
      forkSelf: () =>
        Effect.gen(function* () {
          const before = yield* SelfAgentId.SelfAgentId
          const result = yield* Agents.fork
          const after = yield* SelfAgentId.SelfAgentId
          state.lastFork = JSON.stringify({
            before,
            after,
            tag: result.tag,
            target: uuidToString(result.val.forkedAgentId.uuid),
          })
          return state.lastFork
        }),
    }),
    snapshot: {
      save: (state) =>
        SelfAgentId.SelfAgentId.pipe(
          Effect.map((id) =>
            new TextEncoder().encode(JSON.stringify({ ...state, snapshotIdentity: id })),
          ),
        ),
      restore: (bytes) =>
        Effect.sync(
          () =>
            JSON.parse(new TextDecoder().decode(bytes)) as {
              configured: string
              saved: string
              trace: string[]
              snapshotIdentity: string
              lastFork: string
            },
        ),
    },
  })
}
