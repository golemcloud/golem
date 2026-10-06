import { defineAgent, method, Snapshot } from "@golemcloud/effect-golem"
import { Effect, Ref, Schema } from "effect"

interface IndexState {
  readonly revision: number
  readonly index: Uint8Array
  readonly restored: boolean
}

export const MultipartIndex = defineAgent({
  name: "MultipartIndex",
  id: { name: Schema.String },
  snapshotting: Snapshot.multipart({
    schema: Schema.Struct({ revision: Schema.Number }),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    append: method({ input: { byte: Schema.Number }, success: Schema.Void }),
    inspect: method({ input: {}, success: Schema.String, readOnly: true }),
  },
}).implement({
  init: () =>
    Ref.make<IndexState>({
      revision: 0,
      index: Uint8Array.from({ length: 256 }, (_, i) => i),
      restored: false,
    }),
  methods: (ref) => ({
    append: ({ byte }) =>
      Ref.update(ref, (state) => ({
        ...state,
        revision: state.revision + 1,
        index: new Uint8Array([...state.index, byte]),
      })),
    inspect: () =>
      Ref.get(ref).pipe(
        Effect.map(
          (state) => `${state.revision}|${state.restored}|${Array.from(state.index).join(",")}`,
        ),
      ),
  }),
  snapshot: {
    save: (ref) =>
      Ref.get(ref).pipe(
        Effect.map((state) => ({
          state: { revision: state.revision },
          parts: new Map([
            ["index", { bytes: state.index, contentType: "application/octet-stream" }],
          ]),
        })),
      ),
    restore: (saved) =>
      Effect.gen(function* () {
        const index = yield* Snapshot.requirePart(saved.parts, "index", "application/octet-stream")
        return yield* Ref.make<IndexState>({
          revision: saved.state.revision,
          index,
          restored: true,
        })
      }),
  },
})
