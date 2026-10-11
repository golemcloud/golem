import { defineAgent, method, Snapshot } from '@golemcloud/effect-golem';
import { Effect, Ref, Schema } from 'effect';

const marker = 'revision-0';
interface State { revision: number; index: Uint8Array; restored: boolean }
export const MultipartIndex = defineAgent({
  name: 'MultipartIndex', id: { name: Schema.String },
  snapshotting: Snapshot.multipart({ schema: Schema.Struct({ revision: Schema.Number }), policy: Snapshot.policy.everyN(10) }),
  methods: {
    append: method({ input: { byte: Schema.Number }, success: Schema.Void }),
    inspect: method({ input: {}, success: Schema.String, readOnly: true }),
  },
}).implement({
  init: () => Ref.make<State>({ revision: 0, index: Uint8Array.from({ length: 256 }, (_, i) => i), restored: false }),
  methods: (ref) => ({
    append: ({ byte }) => Ref.update(ref, s => ({ ...s, revision: s.revision + 1, index: new Uint8Array([...s.index, byte]) })),
    inspect: () => Ref.get(ref).pipe(Effect.map(s => `${marker}|${s.revision}|${s.restored}|${Array.from(s.index).join(',')}`)),
  }),
  snapshot: {
    save: (ref) => Ref.get(ref).pipe(Effect.map(s => ({ state: { revision: s.revision }, parts: new Map([
      ['index', { bytes: s.index, contentType: 'application/octet-stream' }],
    ]) }))),
    restore: (saved) => Effect.gen(function* () {
      const index = yield* Snapshot.requirePart(saved.parts, 'index', 'application/octet-stream');
      return yield* Ref.make<State>({ revision: saved.state.revision, index, restored: true });
    }),
  },
});
