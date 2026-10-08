import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
const { publicValueCodec, schemaType } = await import(
  process.env.CODEC_MODULE ?? '../dist/index.mjs'
);

// Exercise the private lossless boundaries without adding production test hooks.
const source = readFileSync(
  process.env.CODEC_SOURCE ?? new URL('../src/index.ts', import.meta.url),
  'utf8',
);
const between = (start, end) =>
  source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)));
const code = [
  between('function restJson(', 'function parseInvocationResultJson('),
  between('class JsonNumberToken {', 'function jsonIntegerToken('),
  between('function isUnicodeScalarString(', 'function compareCodePoints('),
  between('function parseJson(', 'function parseStrictJson('),
  'return { restJson, parseJson };',
].join('\n');
const { restJson, parseJson } = new Function(
  ts.transpileModule(code, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText,
)();

test('private REST writer and parser retain exact numeric tokens', () => {
  const value = {
    methodParameters: {
      kind: 'record',
      value: {
        fields: [
          { kind: 'u64', value: 18446744073709551615n },
          { kind: 'f32', value: -0 },
          { kind: 'string', value: 'quote " and newline\n' },
        ],
      },
    },
  };
  const json = restJson(value);
  assert.match(json, /18446744073709551615/u);
  assert.match(json, /"value":-0/u);
  assert.equal(parseJson(json).methodParameters.value.fields[2].value, 'quote " and newline\n');
  assert.throws(() => parseJson('{"x":1,"x":2}'));
});

test('fused application conversion retains restrictions, error ordering and fresh budgets', () => {
  const byte = schemaType({ tag: 'u8', restrictions: { min: { tag: 'unsigned', val: 7n } } });
  const codec = publicValueCodec({ root: schemaType({ tag: 'list', element: byte }), defs: [] });
  const input = {
    kind: 'list',
    value: {
      elements: [
        { kind: 'u8', value: 7 },
        { kind: 'u8', value: 255 },
      ],
    },
  };
  for (let i = 0; i < 3; i++) assert.deepEqual(codec.application(input), [7, 255]);
  const text = 'x'.repeat(9 * 1024 * 1024);
  const textCodec = publicValueCodec({ root: schemaType({ tag: 'string' }), defs: [] });
  for (let i = 0; i < 2; i++)
    assert.equal(textCodec.application({ kind: 'string', value: text }), text);
  assert.throws(() =>
    codec.application({ kind: 'list', value: { elements: [{ kind: 'u8', value: 6 }] } }),
  );
  const exceptional = publicValueCodec({
    root: schemaType({
      tag: 'record',
      fields: [
        { name: 'float', body: schemaType({ tag: 'f64' }) },
        { name: 'byte', body: schemaType({ tag: 'u8' }) },
      ],
    }),
    defs: [],
  });
  assert.throws(
    () =>
      exceptional.application({
        kind: 'record',
        value: {
          fields: [
            { kind: 'f64', value: { $float: 'nan' } },
            { kind: 'u8', value: 256 },
          ],
        },
      }),
    (error) => error.code === 'validation-error',
  );
});

for (const size of [100, 10000]) {
  test(`codec profile ${size}`, { skip: !process.env.CODEC_BENCH }, () => {
    const values = Array.from({ length: size }, (_, i) => ({ kind: 'u8', value: i % 256 }));
    const request = { methodParameters: { kind: 'list', value: { elements: values } } };
    const json = restJson(request);
    const codec = publicValueCodec({
      root: schemaType({ tag: 'list', element: schemaType({ tag: 'u8' }) }),
      defs: [],
    });
    const operations = {
      restWriter: () => restJson(request),
      losslessParser: () => parseJson(json, true),
      publicApplication: () => codec.application(request.methodParameters),
    };
    assert.equal(operations.publicApplication().length, size);
    for (const [name, operation] of Object.entries(operations)) {
      for (let i = 0; i < 50; i++) operation();
      const samples = [];
      for (let batch = 0; batch < 7; batch++) {
        const start = performance.now();
        for (let i = 0; i < 100; i++) operation();
        samples.push((performance.now() - start) * 10);
      }
      console.log(
        JSON.stringify({
          size,
          name,
          unit: 'µs/op',
          payloadBytes: Buffer.byteLength(json),
          samples,
        }),
      );
    }
  });
}
