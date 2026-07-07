import { z } from 'zod';
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';

// A counter that opts into snapshotting. The `snapshotting` option declares a
// TYPED state schema — only the schema-declared fields of `this` (here `count`)
// are serialized — plus a policy for WHEN to snapshot (every 5 invocations). On
// recovery the executor restores `count` from the last snapshot and replays the
// oplog tail. This is the declarative fluent replacement for overriding
// `save`/`loadSnapshot` in the decorator SDK.
export const CounterWithSnapshot = defineAgent({
  name: 'CounterWithSnapshot',
  id: { name: z.string() },
  http: http.mount('/snapshot-counters/{name}'),
  snapshotting: { state: z.object({ count: z.number() }), policy: { everyNInvocations: 5 } },
  methods: {
    value: method({ input: {}, returns: z.number() }),
    increment: method({ input: {}, returns: z.number(), http: http.post('/increment') }),
  },
});

export const CounterWithSnapshotImpl = CounterWithSnapshot.implement({
  init: () => ({ count: 0 }),
  methods: {
    value() {
      return this.count;
    },
    increment() {
      this.count += 1;
      return this.count;
    },
  },
});
