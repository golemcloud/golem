import { Agents, defineAgent, Ids, method, Snapshot } from '@golemcloud/effect-golem';
import { Effect, Ref, Schema } from 'effect';

const marker = 'revision-0';
interface State { revision: number; index: Uint8Array; restored: boolean; promiseId: typeof Ids.PromiseId.Type }
export const MultipartIndex = defineAgent({
  name: 'MultipartIndex', id: { name: Schema.String },
  snapshotting: Snapshot.multipart({ schema: Schema.Struct({ revision: Schema.Number, promiseId: Ids.PromiseId }), policy: Snapshot.policy.everyN(10) }),
  methods: {
    append: method({ input: { byte: Schema.Number }, success: Schema.Void }),
    inspect: method({ input: {}, success: Schema.String }),
  },
}).implement({
  init: () => Effect.gen(function* () {
    const promiseId = yield* Schema.decodeEffect(Ids.PromiseId)(yield* Agents.Promises.create);
    return yield* Ref.make<State>({ revision: 0, index: Uint8Array.from({ length: 256 }, (_, i) => i), restored: false, promiseId });
  }),
  methods: (ref) => ({
    append: ({ byte }) => Ref.update(ref, s => ({ ...s, revision: s.revision + 1, index: new Uint8Array([...s.index, byte]) })),
    inspect: () => Effect.gen(function* () {
      const s = yield* Ref.get(ref);
      const promiseId = yield* Schema.encodeEffect(Ids.PromiseId)(s.promiseId);
      yield* Agents.Promises.complete(promiseId, new Uint8Array([42])).pipe(
        Effect.asVoid,
        Effect.catchTag('PromiseAlreadyCompletedError', () => Effect.void),
      );
      const payload = yield* Agents.Promises.await(promiseId);
      if (payload[0] !== 42) throw new Error('promise payload mismatch');
      return `${marker}|${s.revision}|${s.restored}|${Array.from(s.index).join(',')}`;
    }),
  }),
  snapshot: {
    save: (ref) => Ref.get(ref).pipe(Effect.map(s => ({ state: { revision: s.revision, promiseId: s.promiseId }, parts: new Map([
      ['index', { bytes: s.index, contentType: 'application/octet-stream' }],
    ]) }))),
    restore: (saved) => Effect.gen(function* () {
      const index = yield* Snapshot.requirePart(saved.parts, 'index', 'application/octet-stream');
      return yield* Ref.make<State>({ revision: saved.state.revision, index, restored: true, promiseId: saved.state.promiseId });
    }),
  },
});
