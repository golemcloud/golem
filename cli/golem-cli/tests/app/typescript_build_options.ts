import { z } from 'zod';
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';

const CounterAgent = defineAgent({
  name: 'CounterAgent',
  id: { name: z.string() },
  http: http.mount('/counters/{name}'),
  methods: {
    value: method({ input: {}, returns: z.string(), http: http.get('/value') }),
    fail: method({ input: {}, returns: z.string(), http: http.get('/fail') }),
  },
});

CounterAgent.implement({
  init: () => ({}),
  methods: {
    value() { return 'readable application success'; },
    fail() {
      const applicationDiagnosticMessage = 'readable application failure';
      throw new Error(applicationDiagnosticMessage);
    },
  },
});
