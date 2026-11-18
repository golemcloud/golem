import { z } from 'zod';
import { defineAgent, method, Result, s } from '@golemcloud/golem-ts-sdk';

defineAgent({
  name: 'TsMethodFailure',
  id: { name: z.string() },
  methods: {
    fail: method({ input: {}, returns: s.s64() }),
    failAsync: method({ input: {}, returns: s.s64() }),
    value: method({ input: {}, returns: s.s64(), readOnly: true }),
    increment: method({ input: {}, returns: s.s64() }),
    declaredError: method({ input: {}, returns: s.result(s.s64(), z.string()) }),
  },
}).implement({
  init: ({ id }) => {
    if (id.name === 'fail-constructor') throw new Error('constructor gave up');
    return { count: 0n };
  },
  methods: {
    fail() {
      this.count += 7n;
      throw new Error('method gave up');
    },
    async failAsync() {
      this.count += 7n;
      await Promise.resolve();
      throw new Error('method gave up');
    },
    value() { return this.count; },
    increment() { return ++this.count; },
    declaredError() { return Result.err('declared error'); },
  },
});
