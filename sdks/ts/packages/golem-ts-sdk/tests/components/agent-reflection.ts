import { AgentId, defineAgent, method } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

defineAgent({
  name: 'Counter',
  id: {},
  snapshotting: { state: z.object({ count: z.number() }) },
  methods: { add: method({ input: { amount: z.number() }, returns: z.number() }) },
}).implement({
  init: () => ({ count: 7 }),
  methods: {
    add({ amount }) {
      if (!(this.getId() instanceof AgentId)) throw new Error('untyped agent identity');
      this.count += amount;
      return this.count;
    },
  },
});
