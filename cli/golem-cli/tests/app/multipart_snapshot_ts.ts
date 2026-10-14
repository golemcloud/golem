import { z } from 'zod';
import { awaitPromise, completePromise, createPromise, defineAgent, method, Snapshot, type PromiseId } from '@golemcloud/golem-ts-sdk';

const marker = 'revision-0';
interface State { revision: number; index: Uint8Array; restored: boolean; promiseId: PromiseId }
export const MultipartIndex = defineAgent({
  name: 'MultipartIndex',
  id: { name: z.string() },
  snapshotting: {
    multipart: { state: z.object({ revision: z.number(), promiseId: z.custom<PromiseId>() }) },
    policy: { everyNInvocations: 10 },
  },
  methods: {
    append: method({ input: { byte: z.number() }, returns: z.void() }),
    inspect: method({ input: {}, returns: z.string() }),
  },
}).implement({
  init: (): State => ({ revision: 0, index: Uint8Array.from({ length: 256 }, (_, i) => i), restored: false, promiseId: createPromise() }),
  methods: {
    append({ byte }) {
      this.revision++;
      this.index = new Uint8Array([...this.index, byte]);
    },
    async inspect() {
      completePromise(this.promiseId, new Uint8Array([42]));
      if ((await awaitPromise(this.promiseId))[0] !== 42) throw new Error('promise payload mismatch');
      return `${marker}|${this.revision}|${this.restored}|${Array.from(this.index).join(',')}`;
    },
  },
  snapshot: {
    async save() {
      return { state: { revision: this.revision, promiseId: this.promiseId }, parts: new Map([
        ['index', { bytes: this.index, contentType: 'application/octet-stream' }],
      ]) };
    },
    async load(saved) {
      return { revision: saved.state.revision, index: Snapshot.requirePart(saved.parts, 'index', 'application/octet-stream'), restored: true, promiseId: saved.state.promiseId };
    },
  },
});
