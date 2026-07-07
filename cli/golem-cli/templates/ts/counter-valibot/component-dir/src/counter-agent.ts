import * as v from 'valibot';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

// The same durable counter as the default template, but authored with Valibot
// schemas instead of Zod. The fluent SDK accepts any Standard Schema vendor, so
// `v.string()` / `v.number()` / `v.void()` drop straight into the contract.
export const Counter = defineAgent({
  name: 'Counter',
  id: { name: v.string() },
  methods: {
    value: method({ input: {}, returns: v.number() }),
    increment: method({ input: {}, returns: v.number() }),
    add: method({ input: { by: v.number() }, returns: v.number() }),
    reset: method({ input: {}, returns: v.void() }),
  },
});

export const CounterImpl = Counter.implement({
  init: () => ({ count: 0 }),
  methods: {
    value() {
      return this.count;
    },
    increment() {
      this.count += 1;
      return this.count;
    },
    add({ by }) {
      this.count += by;
      return this.count;
    },
    reset() {
      this.count = 0;
    },
  },
});
