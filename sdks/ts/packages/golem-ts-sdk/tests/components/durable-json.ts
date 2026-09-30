import { createDurableJsonWriter, defineAgent, method } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

defineAgent({
  name: 'Counter',
  id: {},
  snapshotting: { state: z.object({ count: z.number() }) },
  methods: {
    add: method({ input: { amount: z.number() }, returns: z.number() }),
    append: method({ input: {}, returns: z.void() }),
  },
}).implement({
  init: () => ({ count: 7 }),
  methods: {
    add({ amount }) {
      this.count += amount;
      return this.count;
    },
    async append() {
      const writer = createDurableJsonWriter(z.string(), {
        url: 'http://example.invalid/stream',
        producerId: 'test',
      });
      await writer.dispose();
    },
  },
});
