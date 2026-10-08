import { it } from 'vitest';
import { z } from 'zod';
import '../src/schema/zod';
import { registerAgentType } from '../src/runtime';
import { schemaValueToWit, v } from '../src/internal/schema-model';

for (const size of [1, 64]) {
  const item = z.object({ name: z.string(), count: z.number() });
  const registered = registerAgentType(
    `benchSize${size}`,
    {},
    {
      echo: { input: { items: z.array(item) }, returns: z.array(item) },
    },
  );
  const codec = registered.methodCodecs.get('echo')!.inputCodecs[0].codec;
  const runtime = registered.runtimeMethods.get('echo')!;
  const value = Array.from({ length: size }, (_, i) => ({ name: `item-${i}`, count: i + 7 }));
  const input = schemaValueToWit(v.record([codec.toValue(value)]));
  const operations = {
    decode: () => runtime.read(input, { tag: 'anonymous' }),
    encode: () => runtime.write(value),
    invoke: () => runtime.write(runtime.read(input, { tag: 'anonymous' }).items),
  };
  for (const [name, operation] of Object.entries(operations)) {
    it.skipIf(!process.env.CODEC_BENCH)(
      `${size} records: ${name}`,
      async () => {
        for (let i = 0; i < 1000; i++) await operation();
        const samples = [];
        for (let batch = 0; batch < 7; batch++) {
          const start = performance.now();
          for (let i = 0; i < 2000; i++) await operation();
          samples.push(((performance.now() - start) * 1000) / 2000);
        }
        console.log(JSON.stringify({ size, name, unit: 'µs/op', samples }));
      },
      60000,
    );
  }
}
