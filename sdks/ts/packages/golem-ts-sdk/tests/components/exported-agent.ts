import { defineAgent, method, s } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

export const Counter = defineAgent({
  name: 'Counter',
  id: {},
  snapshotting: { state: z.object({ count: z.number() }) },
  methods: {
    add: method({ input: { amount: z.number() }, returns: z.number() }),
    echo: method({
      input: {
        value: s.multimodal([
          { name: 'text', schema: s.unstructuredText() },
          { name: 'binary', schema: s.unstructuredBinary() },
        ]),
      },
      returns: z.void(),
    }),
  },
});

export const CounterImpl = Counter.implement({
  init: () => ({ count: 7 }),
  methods: {
    add({ amount }) {
      this.count += amount;
      return this.count;
    },
    echo() {},
  },
});
