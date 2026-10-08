import { defineAgent, method, s } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

const ConversionBenchTs = defineAgent({
  name: 'ConversionBenchTs',
  id: { name: z.string() },
  methods: {
    checksum: method({ input: { input: s.uint8Array() }, returns: s.u32() }),
    produce: method({ input: { length: s.u32() }, returns: s.uint8Array() }),
  },
});

ConversionBenchTs.implement({
  init: () => ({}),
  methods: {
    checksum({ input }) { return input.reduce((sum, byte) => (sum + byte) >>> 0, 0); },
    produce({ length }) { return Uint8Array.from({ length }, (_, i) => i % 251); },
  },
});
