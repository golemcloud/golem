import { defineAgent, method } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

const value = [7, 5, 3].reduce((sum, item) => sum * 2 + item, 0);

defineAgent({
  name: 'JsGuestSmoke',
  description: `TS initialized value: ${value}, clock: ${Date.now()}`,
  id: {},
  methods: { get: method({ input: {}, returns: z.number() }) },
}).implement({
  init: () => ({}),
  methods: { get: () => value },
});
