import { Effect, Schema } from "effect"
import { uuidToString } from "golem:core/types@2.0.0"
import {
  AgentIdentity,
  Agents,
  Oplog,
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
      checkpoint: method({ input: {}, success: Schema.String }),
      forkNamed: method({
        input: { name: Schema.String, cutoff: Schema.String },
        success: Schema.Void,
      }),
      forkSelf: method({ input: {}, success: Schema.String }),
    },
  })
  spec.implement({
    init: ({ name }) =>
      SelfAgentId.SelfAgentId.pipe(
        Effect.map((id) => ({
          configured: name,
          saved: id.agentId,
          trace: [] as string[],
          snapshotIdentity: "",
          lastFork: "",
        })),
      ),
    methods: (state) => ({
      observe: () =>
        Effect.gen(function* () {
          const self = yield* SelfAgentId.SelfAgentId
          // Different durable calls make an incorrect historical identity diverge.
          if (self.agentId.includes('"parent"')) yield* Agents.getSelfMetadata
          else yield* Oplog.currentIndex
          state.trace.push(self.agentId)
          return self.agentId
        }),
      status: () =>
        Effect.gen(function* () {
          const self = yield* SelfAgentId.SelfAgentId
          const metadata = yield* Agents.getSelfMetadata
          const addressed = yield* Agents.getAgentMetadata(self)
          return JSON.stringify({
            ...state,
            current: self.agentId,
            metadata: metadata.agentId.agentId,
            addressed: addressed?.agentId.agentId,
          })
        }),
      checkpoint: () => Oplog.currentIndex.pipe(Effect.map(String)),
      forkNamed: ({ name, cutoff }) =>
        Effect.gen(function* () {
          const source = yield* SelfAgentId.SelfAgentId
          const target = yield* spec.agentId({ name })
          yield* Agents.forkAgent({
            source,
            target: { ...source, agentId: target.encoded },
            oplogIdxCutOff: BigInt(cutoff),
          })
        }),
      forkSelf: () =>
        Effect.gen(function* () {
          const before = yield* SelfAgentId.SelfAgentId
          const result = yield* Agents.fork
          const after = yield* SelfAgentId.SelfAgentId
          const identity = yield* AgentIdentity.parse(before.agentId)
          const target = yield* AgentIdentity.make({
            typeName: identity.typeName,
            constructorValue: identity.constructorValue,
            phantomId: uuidToString(result.val.forkedPhantomId),
          })
          state.lastFork = JSON.stringify({
            before: before.agentId,
            after: after.agentId,
            tag: result.tag,
            target: target.encoded,
          })
          return state.lastFork
        }),
    }),
    snapshot: {
      save: (state) =>
        SelfAgentId.SelfAgentId.pipe(
          Effect.map((id) =>
            new TextEncoder().encode(JSON.stringify({ ...state, snapshotIdentity: id.agentId })),
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
