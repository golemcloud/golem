import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

// A minimal durable counter agent in the fluent (Standard Schema) SDK:
// `defineAgent(...)` declares the contract, `.implement(...)` supplies handlers
// whose `this` is bound to the state returned by `init`.
export const CounterAgent = defineAgent({
  name: 'CounterAgent',
  id: { name: z.string() },
  methods: {
    value: method({ input: {}, returns: z.number() }),
    increment: method({ input: {}, returns: z.number() }),
    add: method({ input: { by: z.number() }, returns: z.number() }),
    reset: method({ input: {}, returns: z.void() }),
  },
});

export const CounterAgentImpl = CounterAgent.implement({
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
