import { z } from 'zod';
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';

// A minimal durable counter agent in the schema-driven TypeScript SDK:
// `defineAgent(...)` declares the contract, `.implement(...)` supplies handlers
// whose `this` is bound to the state returned by `init`.
export const CounterAgent = defineAgent({
  name: 'CounterAgent',
  id: { name: z.string() },
  http: http.mount('/counters/{name}'),
  methods: {
    value: method({ input: {}, returns: z.number(), http: http.get('/value') }),
    increment: method({
      input: {},
      returns: z.number(),
      http: http.post('/increment'),
    }),
    add: method({
      input: { by: z.number() },
      returns: z.number(),
      http: http.post('/add'),
    }),
    reset: method({ input: {}, returns: z.void(), http: http.post('/reset') }),
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
