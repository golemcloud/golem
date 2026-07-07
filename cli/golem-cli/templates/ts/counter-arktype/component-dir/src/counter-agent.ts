import { type } from 'arktype';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

// The same durable counter as the default template, but authored with ArkType
// schemas instead of Zod. The fluent SDK accepts any Standard Schema vendor, so
// `type('string')` / `type('number')` drop straight into the contract.
//
// ArkType has no first-class `void`, so `reset` returns the reset value (0)
// instead of nothing — keeping every method's return schema a real ArkType type.
export const Counter = defineAgent({
  name: 'Counter',
  id: { name: type('string') },
  methods: {
    value: method({ input: {}, returns: type('number') }),
    increment: method({ input: {}, returns: type('number') }),
    add: method({ input: { by: type('number') }, returns: type('number') }),
    reset: method({ input: {}, returns: type('number') }),
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
      return this.count;
    },
  },
});
