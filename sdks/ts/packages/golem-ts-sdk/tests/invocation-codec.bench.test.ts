import { expect, it } from 'vitest';
import { z } from 'zod';
import '../src/schema/zod';
import { s } from '../src/schema/markers';
import { compileSchema } from '../src/schema/adapter';
import { SchemaValueWriter, invocationSchemaValueReader } from '../src/schema/codec';
import { schemaValueFromWit, schemaValueToWit } from '../src/internal/schema-model';

for (const size of [100, 10000]) {
  it.skipIf(!process.env.CODEC_BENCH)(
    `invocation codecs ${size}`,
    () => {
      const codec = compileSchema(z.object({ bytes: z.array(s.u8()), label: z.string() }));
      const value = { bytes: Array.from({ length: size }, (_, i) => i % 256), label: 'asymmetric' };
      const tree = schemaValueToWit(codec.toValue(value));
      const direct = codec.invocationDirect!;
      const operations = {
        ordinaryEncode: () => schemaValueToWit(codec.toValue(value)),
        directEncode: () => {
          const writer = new SchemaValueWriter();
          const root = direct.write(value, writer)!;
          return { valueNodes: writer.valueNodes, root };
        },
        ordinaryDecode: () => codec.fromValue(schemaValueFromWit(tree)),
        directDecode: () => direct.read(invocationSchemaValueReader(tree), tree.root),
      };
      expect(operations.directEncode()).toEqual(tree);
      expect(operations.directDecode()).toEqual(value);
      for (const [name, operation] of Object.entries(operations)) {
        for (let i = 0; i < 50; i++) operation();
        const samples = [];
        for (let batch = 0; batch < 7; batch++) {
          const start = performance.now();
          for (let i = 0; i < 100; i++) operation();
          samples.push((performance.now() - start) * 10);
        }
        console.log(
          JSON.stringify({ size, name, unit: 'µs/op', nodes: tree.valueNodes.length, samples }),
        );
      }
    },
    60000,
  );
}
