import { z } from 'zod';
import { defineAgent, method, Snapshot } from '@golemcloud/golem-ts-sdk';

const marker = 'revision-0';
interface State { revision: number; index: Uint8Array; restored: boolean }
export const MultipartIndex = defineAgent({
  name: 'MultipartIndex',
  id: { name: z.string() },
  snapshotting: {
    multipart: { state: z.object({ revision: z.number() }) },
    policy: { everyNInvocations: 10 },
  },
  methods: {
    append: method({ input: { byte: z.number() }, returns: z.void() }),
    inspect: method({ input: {}, returns: z.string() }),
  },
}).implement({
  init: (): State => ({ revision: 0, index: Uint8Array.from({ length: 256 }, (_, i) => i), restored: false }),
  methods: {
    append({ byte }) {
      this.revision++;
      this.index = new Uint8Array([...this.index, byte]);
    },
    inspect() {
      return `${marker}|${this.revision}|${this.restored}|${Array.from(this.index).join(',')}`;
    },
  },
  snapshot: {
    async save() {
      return { state: { revision: this.revision }, parts: new Map([
        ['index', { bytes: this.index, contentType: 'application/octet-stream' }],
      ]) };
    },
    async load(saved) {
      return { revision: saved.state.revision, index: Snapshot.requirePart(saved.parts, 'index', 'application/octet-stream'), restored: true };
    },
  },
});
