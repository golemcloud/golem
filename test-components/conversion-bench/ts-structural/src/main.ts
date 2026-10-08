import { defineAgent, method, s } from '@golemcloud/golem-ts-sdk';
import { z } from 'zod';

// The SDK walker accepts marker children; Zod's constructor type only accepts Zod schemas.
const bytes = z.array(s.u8() as unknown as z.ZodType<number>);

const ConversionBenchTsStructural = defineAgent({
  name: 'ConversionBenchTsStructural',
  id: { name: z.string() },
  methods: {
    checksum: method({ input: { input: bytes }, returns: s.u32() }),
    produce: method({ input: { length: s.u32() }, returns: bytes }),
  },
});

ConversionBenchTsStructural.implement({
  init: () => ({}),
  methods: {
    checksum({ input }) { return input.reduce((sum, byte) => (sum + byte) >>> 0, 0); },
    produce({ length }) { return Array.from({ length }, (_, i) => i % 251); },
  },
});
