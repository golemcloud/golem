import assert from 'node:assert/strict';
import test from 'node:test';
import { once } from 'node:events';
import { readFile } from 'node:fs/promises';
import { createServer as createHttpServer } from 'node:http';
import { WebSocketServer } from 'ws';
import {
  agentStream,
  createAgent,
  createStreamingRemoteMethod,
  decodeStreamSessionEnd,
  Datetime,
  encodeOption,
  encodeStreamSessionBinaryEnvelope,
  encodeStreamSessionTextFrame,
  getAgent,
  GolemConflictError,
  GolemServiceError,
  invokeAgent,
  lookupAgent,
  parseStreamSessionTextFrame,
  publicValueCodec,
  schemaGraphFromWire,
  schemaType,
  StreamingProtocolError,
  StreamingUnreachableError,
  upsertAgent,
  Uuid,
} from '../dist/index.mjs';
import { createServer as createTcpServer } from 'node:net';

const outputMapping = {
  channel: 2,
  direction: 'output',
  streamToken: 'output',
};
const u32Graph = { root: { kind: 'u32', value: {} } };
const unitGraph = { root: { kind: 'tuple', value: { elements: [] } } };
const outputStreamGraph = { root: { kind: 'stream', value: { inner: null } } };
const u32ListGraph = {
  root: { kind: 'list', value: { element: { kind: 'u32', value: {} } } },
};
const integerRecordGraph = {
  root: {
    kind: 'record',
    value: {
      fields: [
        { name: 'unsigned', body: { kind: 'u64', value: {} } },
        { name: 'signed', body: { kind: 's64', value: {} } },
        {
          name: 'nested',
          body: {
            kind: 'tuple',
            value: {
              elements: [
                { kind: 's64', value: {} },
                {
                  kind: 'record',
                  value: {
                    fields: [
                      { name: 'small', body: { kind: 'u64', value: {} } },
                      { name: 'float', body: { kind: 'f64', value: {} } },
                      { name: 'text', body: { kind: 'string', value: {} } },
                    ],
                  },
                },
              ],
            },
          },
        },
      ],
    },
  },
};

// The bridge counts an accepted connection as progress once it stays up this long.
const STABLE_CONNECTION_MS = 5_000;

test('option-none uses an explicit native null payload', () => {
  assert.deepEqual(
    encodeOption(undefined, () => assert.fail('none must not encode an item')),
    {
      kind: 'option',
      value: { inner: null },
    },
  );
});

test('config application JSON preserves ordinary reserved-looking fields and nested sums', () => {
  const graph = schemaGraphFromWire({
    root: {
      kind: 'record',
      value: {
        fields: [
          { name: 'kind', body: { kind: 'string', value: {} } },
          {
            name: 'value',
            body: {
              kind: 'option',
              value: {
                inner: {
                  kind: 'result',
                  value: { spec: { ok: { kind: 'string', value: {} } } },
                },
              },
            },
          },
          { name: '$option', body: { kind: 'string', value: {} } },
          { name: '$result', body: { kind: 'string', value: {} } },
        ],
      },
    },
  });
  const value = {
    kind: 'record',
    value: {
      fields: [
        { kind: 'string', value: 'application-field' },
        {
          kind: 'option',
          value: {
            inner: {
              kind: 'result',
              value: { tag: 'ok', value: { kind: 'string', value: 'nested' } },
            },
          },
        },
        { kind: 'string', value: 'ordinary-option-field' },
        { kind: 'string', value: 'ordinary-result-field' },
      ],
    },
  };

  assert.deepEqual(publicValueCodec(graph).application(value), {
    kind: 'application-field',
    value: { ok: 'nested' },
    $option: 'ordinary-option-field',
    $result: 'ordinary-result-field',
  });
});

test('Datetime is the TypeScript SDK Datetime', async () => {
  const [bridgeSource, sdkSource] = await Promise.all([
    readFile(new URL('../src/datetime.ts', import.meta.url), 'utf8'),
    readFile(new URL('../../golem-ts-sdk/src/datetime.ts', import.meta.url), 'utf8'),
  ]);
  assert.equal(bridgeSource, sdkSource);
});

test('Datetime parses any RFC 3339 offset and emits canonical UTC text', () => {
  const datetime = Datetime.parse('2024-03-01T01:04:56.120+02:30');
  assert.deepEqual(datetime, { seconds: 1709246096n, nanoseconds: 120000000 });
  assert.equal(Datetime.toISOString(datetime), '2024-02-29T22:34:56.12Z');
  assert.equal(
    Datetime.toISOString({ seconds: 253402300799n, nanoseconds: 999999999 }),
    '9999-12-31T23:59:59.999999999Z',
  );
  assert.throws(() => Datetime.parse('2024-01-01T00:00:00'), /Invalid RFC 3339 datetime/);
  assert.throws(
    () => Datetime.parse('0000-01-01T00:00:00+00:01'),
    /outside the years 0000\.\.9999/,
  );
  assert.throws(
    () => Datetime.toISOString({ seconds: 253402300800n, nanoseconds: 0 }),
    /outside the years 0000\.\.9999/,
  );
  assert.deepEqual(Datetime.fromDate(new Date(-1)), { seconds: -1n, nanoseconds: 999000000 });
  assert.equal(Datetime.toDate({ seconds: 1n, nanoseconds: 2_999_999 }).getTime(), 1002);
});

test('config application JSON canonicalizes f32 and datetime and rejects exceptional floats', () => {
  assert.equal(
    publicValueCodec(schemaGraphFromWire({ root: { kind: 'f32', value: {} } })).application({
      kind: 'f32',
      value: 0.1,
    }),
    Math.fround(0.1),
  );
  assert.equal(
    publicValueCodec(schemaGraphFromWire({ root: { kind: 'datetime', value: {} } })).application({
      kind: 'datetime',
      value: { value: '2026-01-01T00:00:00.12Z' },
    }),
    '2026-01-01T00:00:00.120000000Z',
  );
  const exceptional = publicValueCodec(
    schemaGraphFromWire({
      root: {
        kind: 'option',
        value: { inner: { kind: 'f64', value: {} } },
      },
    }),
  );
  const value = {
    kind: 'option',
    value: { inner: { kind: 'f64', value: { $float: 'nan' } } },
  };
  assert.deepEqual(exceptional.validate(value, 'none'), value);
  assert.throws(() => exceptional.application(value), /no application JSON representation/);
});

test('Uuid parses, formats and round-trips its 64-bit halves', () => {
  const uuid = Uuid.parse('DD00721B-3329-4621-A01D-C71F02CD78C6');
  assert.equal(uuid.toString(), 'dd00721b-3329-4621-a01d-c71f02cd78c6');
  assert.equal(uuid.highBits, 0xdd00721b33294621n);
  assert.equal(uuid.lowBits, 0xa01dc71f02cd78c6n);
  assert.equal(
    Uuid.from({ highBits: uuid.highBits, lowBits: uuid.lowBits }).toString(),
    uuid.toString(),
  );
  assert.equal(Uuid.from(uuid), uuid);
  assert.equal(new Uuid(0n, 0n).toString(), '00000000-0000-0000-0000-000000000000');
  assert.equal(
    new Uuid((1n << 64n) - 1n, (1n << 64n) - 1n).toString(),
    'ffffffff-ffff-ffff-ffff-ffffffffffff',
  );
  assert.match(
    Uuid.generate().toString(),
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u,
  );
  assert.throws(() => Uuid.parse('not-a-uuid'), /Invalid UUID/);
  assert.throws(() => Uuid.parse('dd00721b33294621a01dc71f02cd78c6'), /Invalid UUID/);
  assert.throws(() => new Uuid(-1n, 0n), RangeError);
  assert.throws(() => new Uuid(0n, 1n << 64n), RangeError);
});

test('uuid values validate as canonical strings and map to application JSON strings', () => {
  const codec = publicValueCodec(
    schemaGraphFromWire({
      root: { kind: 'list', value: { element: { kind: 'uuid', value: {} } } },
    }),
  );
  const value = {
    kind: 'list',
    value: { elements: [{ kind: 'uuid', value: 'dd00721b-3329-4621-a01d-c71f02cd78c6' }] },
  };
  assert.deepEqual(codec.validate(value, 'none'), value);
  assert.deepEqual(codec.application(value), ['dd00721b-3329-4621-a01d-c71f02cd78c6']);
  assert.throws(
    () =>
      codec.validate(
        {
          kind: 'list',
          value: { elements: [{ kind: 'uuid', value: 'DD00721B-3329-4621-A01D-C71F02CD78C6' }] },
        },
        'none',
      ),
    /lowercase hyphenated UUID/,
  );
});
const numericFormsGraph = {
  root: {
    kind: 'record',
    value: {
      fields: ['u64', 's64', 'u64', 's64', 'u64', 'f64'].map((kind, index) => ({
        name: `value${index}`,
        body: { kind, value: {} },
      })),
    },
  },
};

const fixtureType = (input) => {
  const type = (body) => schemaType(body);
  switch (input.kind) {
    case 'ref':
      return type({ tag: 'ref', id: input.name });
    case 'bool':
    case 'char':
    case 'string':
    case 'uuid':
    case 'datetime':
    case 'duration':
      return type({ tag: input.kind });
    case 's8':
    case 's16':
    case 's32':
    case 's64':
    case 'u8':
    case 'u16':
    case 'u32':
    case 'u64':
    case 'f32':
    case 'f64':
      return type({ tag: input.kind });
    case 'text':
      return type({ tag: 'text', restrictions: {} });
    case 'binary':
      return type({
        tag: 'binary',
        restrictions: {
          ...(input.mimeTypes === undefined ? {} : { mimeTypes: input.mimeTypes }),
          ...(input.minBytes === undefined ? {} : { minBytes: input.minBytes }),
          ...(input.maxBytes === undefined ? {} : { maxBytes: input.maxBytes }),
        },
      });
    case 'path':
      return type({ tag: 'path', spec: { direction: 'input', kind: 'any' } });
    case 'url':
      return type({ tag: 'url', restrictions: {} });
    case 'quantity':
      return type({ tag: 'quantity', spec: { baseUnit: 'kg', allowedSuffixes: [] } });
    case 'record':
      return type({
        tag: 'record',
        fields: input.fields.map((field) => ({
          name: field.name,
          body: fixtureType(field.type),
          metadata: { aliases: [], examples: [] },
        })),
      });
    case 'tuple':
      return type({ tag: 'tuple', elements: input.elements.map(fixtureType) });
    case 'list':
      return type({ tag: 'list', element: fixtureType(input.element) });
    case 'fixed-list':
      return type({
        tag: 'fixed-list',
        element: fixtureType(input.element),
        length: input.length,
      });
    case 'map':
      return type({ tag: 'map', key: fixtureType(input.key), value: fixtureType(input.value) });
    case 'enum':
      return type({ tag: 'enum', cases: input.cases });
    case 'flags':
      return type({ tag: 'flags', names: input.flags });
    case 'variant':
      return type({
        tag: 'variant',
        cases: input.cases.map((item) => ({
          name: item.name,
          ...(item.type === undefined ? {} : { payload: fixtureType(item.type) }),
          metadata: { aliases: [], examples: [] },
        })),
      });
    case 'option':
      return type({ tag: 'option', element: fixtureType(input.inner) });
    case 'result':
      return type({
        tag: 'result',
        ...(input.ok == null ? {} : { ok: fixtureType(input.ok) }),
        ...(input.err == null ? {} : { err: fixtureType(input.err) }),
      });
    case 'union':
      return type({
        tag: 'union',
        branches: input.branches.map((branch) => ({
          tag: branch.name,
          body: fixtureType(branch.type),
          discriminator: { tag: 'prefix', val: branch.discriminator.prefix },
          metadata: { aliases: [], examples: [] },
        })),
      });
    case 'stream':
      return type({
        tag: 'stream',
        ...(input.inner == null ? {} : { element: fixtureType(input.inner) }),
      });
    default:
      throw new Error(`Unknown fixture schema kind '${input.kind}'`);
  }
};

const fixtureGraph = (vector) => ({
  root: fixtureType(vector.schema),
  defs: new Map(
    Object.entries(vector.definitions ?? {}).map(([id, body]) => [
      id,
      { name: id, body: fixtureType(body) },
    ]),
  ),
});

const fixtureStreamPolicy = (text) =>
  text.includes('provisionalRef')
    ? 'provisional'
    : text.includes('streamToken')
      ? 'stable'
      : 'none';

test('native value codec matches every frozen shared schema-value fixture', async () => {
  const fixture = JSON.parse(
    await readFile('../../../../golem-client/tests/fixtures/stream-session-v1/schema-values.json'),
  );
  for (const vector of fixture.vectors) {
    const value = parseStreamSessionTextFrame(vector.canonical);
    const validated = publicValueCodec(fixtureGraph(vector)).validate(
      value,
      fixtureStreamPolicy(vector.canonical),
    );
    assert.equal(encodeStreamSessionTextFrame(validated), vector.canonical, vector.name);
  }
});

test('native value codec rejects every frozen malformed schema-value fixture', async () => {
  const fixture = JSON.parse(
    await readFile('../../../../golem-client/tests/fixtures/stream-session-v1/malformed.json'),
  );
  for (const vector of fixture.vectors.filter(({ lane }) => lane === 'schema-value')) {
    const value = parseStreamSessionTextFrame(vector.input);
    assert.throws(
      () =>
        publicValueCodec(fixtureGraph(vector)).validate(value, fixtureStreamPolicy(vector.input)),
      { code: vector.expectedCode },
      vector.name,
    );
  }
});

test('binary envelope encoder matches the frozen shared fixtures', async () => {
  const fixture = JSON.parse(
    await readFile(
      '../../../../golem-client/tests/fixtures/stream-session-v1/binary-messages.json',
    ),
  );
  for (const vector of fixture.vectors) {
    const metadata = JSON.parse(vector.metadata);
    const actual = encodeStreamSessionBinaryEnvelope(
      metadata,
      Buffer.from(vector.payloadHex, 'hex'),
    );
    assert.equal(actual.toString('base64'), vector.frameBase64, vector.name);
  }
});

test('text codec matches the frozen canonical JSON messages', async () => {
  const fixture = JSON.parse(
    await readFile('../../../../golem-client/tests/fixtures/stream-session-v1/json-messages.json'),
  );
  for (const vector of fixture.vectors) {
    const parsed = parseStreamSessionTextFrame(vector.canonical);
    if (parsed.type === 'stream-end') decodeStreamSessionEnd(parsed);
    assert.equal(encodeStreamSessionTextFrame(parsed), vector.canonical, vector.name);
  }
});

test('strict text codec directly rejects frozen malformed JSON syntax', async () => {
  const fixture = JSON.parse(
    await readFile('../../../../golem-client/tests/fixtures/stream-session-v1/malformed.json'),
  );
  const parserOwned = new Set(['invalid-json', 'duplicate-top-level-field']);
  for (const vector of fixture.vectors.filter(({ name }) => parserOwned.has(name))) {
    const input = vector.input ?? Buffer.from(vector.inputBase64, 'base64').toString('utf8');
    assert.throws(
      () => parseStreamSessionTextFrame(input),
      { code: vector.expectedCode },
      vector.name,
    );
  }
});

test('terminal decoder rejects every shared malformed terminal vector', async () => {
  const fixture = JSON.parse(
    await readFile('../../../../golem-client/tests/fixtures/stream-session-v1/malformed.json'),
  );
  for (const vector of fixture.vectors.filter(
    ({ lane, input, state }) => lane === 'text' && input?.includes('stream-end') && !state,
  )) {
    assert.throws(
      () => decodeStreamSessionEnd(parseStreamSessionTextFrame(vector.input)),
      { code: vector.expectedCode },
      vector.name,
    );
  }
  const frame = {
    version: 1,
    type: 'stream-end',
    channel: 2,
    direction: 'output',
    sequence: '0',
    outcome: { kind: 'finished' },
  };
  for (const invalid of [
    { ...frame, direction: undefined },
    { ...frame, direction: 'sideways' },
    { ...frame, type: 'outputStreamEnd' },
    { ...frame, outcome: { kind: 'finished', error: {} } },
    { ...frame, outcome: { kind: 'consumer-cancelled', details: 42 } },
    { ...frame, outcome: { kind: 'failed', error: { kind: 'trapped' } } },
    { ...frame, outcome: { kind: 'failed', error: { kind: 'cancelled', message: 'extra' } } },
  ])
    assert.throws(() => decodeStreamSessionEnd(invalid), { code: 'malformed-message' });
});

const terminalOutcomes = [
  { kind: 'finished' },
  {
    kind: 'failed',
    error: { kind: 'failed', value: { graph: u32Graph, value: { kind: 'u32', value: 7 } } },
  },
  ...['trapped', 'interrupted', 'resource-exhausted'].map((kind) => ({
    kind: 'failed',
    error: { kind, message: `${kind} diagnostic` },
  })),
  ...['cancelled', 'abandoned'].map((kind) => ({ kind: 'failed', error: { kind } })),
  { kind: 'consumer-cancelled' },
  { kind: 'consumer-cancelled', details: null },
  { kind: 'consumer-cancelled', details: 'consumer diagnostic' },
];

for (const outcome of terminalOutcomes) {
  test(`output terminal preserves complete outcome ${JSON.stringify(outcome)}`, async () => {
    const local = await server((socket) => {
      socket.once('message', (data) => {
        const start = JSON.parse(data);
        socket.send(JSON.stringify(accepted(start, [outputMapping])));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: {
              kind: 'value',
              graph: outputStreamGraph,
              value: { kind: 'stream', value: { streamToken: 'output' } },
            },
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'stream-end',
            direction: 'output',
            channel: 2,
            sequence: '0',
            outcome,
          }),
        );
        socket.send(
          JSON.stringify({ version: 1, type: 'invocationFinished', outcome: { kind: 'success' } }),
        );
      });
    });
    try {
      const method = createStreamingRemoteMethod(
        () => ({ server: local.endpoint }),
        () => descriptor,
        () => ({}),
        (value, stream) => stream(value.value.value.streamToken, (item) => item, 'terminal-test'),
      );
      const output = await method();
      const iterator = output[Symbol.asyncIterator]();
      if (outcome.kind === 'finished')
        assert.deepEqual(await iterator.next(), { done: true, value: undefined });
      else
        await assert.rejects(iterator.next(), (error) => {
          assert.ok(error instanceof StreamingProtocolError);
          assert.deepEqual(error.outcome, outcome);
          assert.equal(error.code, outcome.error?.kind ?? outcome.kind);
          return true;
        });
    } finally {
      await local.close();
    }
  });
}

const MiB = 1024 * 1024;

function typedTerminalFrame(channel, bytes, sequence = '0') {
  const frame = {
    version: 1,
    type: 'stream-end',
    direction: 'output',
    channel,
    sequence,
    outcome: {
      kind: 'failed',
      error: {
        kind: 'failed',
        value: {
          graph: { root: { kind: 'string', value: {} } },
          value: { kind: 'string', value: '' },
        },
      },
    },
  };
  const remaining = bytes - Buffer.byteLength(JSON.stringify(frame));
  frame.outcome.error.value.value.value =
    'é'.repeat(Math.floor(remaining / 2)) + 'x'.repeat(remaining % 2);
  assert.equal(Buffer.byteLength(JSON.stringify(frame)), bytes);
  return frame;
}

async function terminalBudgetSession(count) {
  let socket;
  const mappings = Array.from({ length: count + 1 }, (_, index) => ({
    channel: index + 1,
    direction: 'output',
    streamToken: `budget-${index}`,
  }));
  const local = await server((connected) => {
    socket = connected;
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(JSON.stringify(accepted(start, mappings)));
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: { root: { kind: 'list', value: { element: outputStreamGraph.root } } },
            value: {
              kind: 'list',
              value: {
                elements: mappings.map(({ streamToken }) => ({
                  kind: 'stream',
                  value: { streamToken },
                })),
              },
            },
          },
        }),
      );
    });
  });
  const method = createStreamingRemoteMethod(
    () => ({ server: local.endpoint }),
    () => descriptor,
    () => ({}),
    (value, stream) =>
      value.value.value.elements.map((item) =>
        stream(item.value.streamToken, (item) => item)[Symbol.asyncIterator](),
      ),
  );
  const controller = new AbortController();
  const outputs = await method.abortable(controller.signal);
  const marker = outputs.pop();
  let sequence = 0;
  const markerFrame = () => ({
    version: 1,
    type: 'outputStreamItem',
    channel: count + 1,
    sequence: String(sequence),
    cursorToken: `budget-cursor-${sequence}`,
    mappings: [],
    value: 'marker',
  });
  const markerBytes = Buffer.byteLength(JSON.stringify(markerFrame()));
  const barrier = () => {
    const frame = markerFrame();
    sequence += 1;
    socket.send(JSON.stringify(frame));
    return marker.next();
  };
  return {
    outputs,
    send: (frame) => socket.send(JSON.stringify(frame)),
    barrier,
    markerBytes,
    close: async () => {
      controller.abort();
      await local.close();
    },
  };
}

test('terminal budget rejects unread prefix plus large typed terminal', async () => {
  const session = await terminalBudgetSession(1);
  try {
    const prefix = {
      version: 1,
      type: 'outputStreamItem',
      channel: 1,
      sequence: '0',
      cursorToken: 'prefix',
      mappings: [],
      value: 'p'.repeat(8 * MiB),
    };
    session.send(prefix);
    session.send(
      typedTerminalFrame(1, 16 * MiB - Buffer.byteLength(JSON.stringify(prefix)) + 1, '1'),
    );
    await assert.rejects(session.barrier(), {
      code: 'resource-exhausted',
      message: 'output delivery queue limit exceeded',
    });
  } finally {
    await session.close();
  }
});

test('terminal budget rejects combined typed terminals above the session limit', async () => {
  const session = await terminalBudgetSession(3);
  try {
    for (let channel = 1; channel <= 3; channel += 1)
      session.send(typedTerminalFrame(channel, channel === 3 ? 8 * MiB + 1 : 12 * MiB));
    await assert.rejects(session.barrier(), {
      code: 'resource-exhausted',
      message: 'session output delivery queue limit exceeded',
    });
  } finally {
    await session.close();
  }
});

test('terminal budget releases full UTF-8 charge on delivery and disposal without changing typed payload', async () => {
  const session = await terminalBudgetSession(4);
  try {
    const frames = [1, 2, 3, 4].map((channel) =>
      typedTerminalFrame(channel, 16 * MiB - (channel % 2 === 0 ? session.markerBytes : 0)),
    );
    session.send(frames[0]);
    session.send(frames[1]);
    assert.equal((await session.barrier()).value, 'marker');
    await assert.rejects(session.outputs[0].next(), (error) => {
      assert.deepEqual(error.outcome, frames[0].outcome);
      return error.code === 'failed';
    });
    session.send(frames[2]);
    assert.equal((await session.barrier()).value, 'marker');
    await session.outputs[1].return();
    session.send(frames[3]);
    assert.equal((await session.barrier()).value, 'marker');
    for (const index of [2, 3])
      await assert.rejects(session.outputs[index].next(), (error) => {
        assert.deepEqual(error.outcome, frames[index].outcome);
        return error.code === 'failed';
      });
    assert.deepEqual(await session.outputs[0].next(), { done: true, value: undefined });
    assert.equal((await session.barrier()).value, 'marker');
  } finally {
    await session.close();
  }
});

for (const count of [1, 2]) {
  test(`input diagnostic budget rejects ${count === 1 ? 'oversized diagnostic' : 'combined unacknowledged diagnostics'}`, async () => {
    const local = await server((socket) => {
      socket.once('message', (data) => {
        const start = JSON.parse(data);
        socket.send(
          JSON.stringify(
            accepted(
              start,
              start.methodParameters.inputs.map((input, index) => ({
                channel: index + 1,
                direction: 'input',
                streamToken: `input-${index}`,
                provisionalRef: input.value.provisionalRef,
                inputHighWater: { sequence: '0', terminal: false },
              })),
            ),
          ),
        );
      });
    });
    try {
      const diagnostic = 'é'.repeat(((count === 1 ? 16 : 9) * MiB) / 2);
      const inputs = Array.from({ length: count }, () =>
        agentStream({
          [Symbol.asyncIterator]() {
            return {
              next: async () => {
                throw new Error(diagnostic);
              },
            };
          },
        }),
      );
      const method = createStreamingRemoteMethod(
        () => ({ server: local.endpoint }),
        () => descriptor,
        ([inputs], register) => ({
          inputs: inputs.map((input) => register(input, (value) => value)),
        }),
        () => undefined,
      );
      await assert.rejects(method(inputs), {
        code: 'resource-exhausted',
        message:
          count === 1
            ? 'input logical value limit exceeded'
            : 'unacknowledged input limit exceeded',
      });
    } finally {
      await local.close();
    }
  });
}

for (const mode of ['synchronous', 'delayed', 'combined']) {
  test(`packed speculative diagnostic budget rejects ${mode} failure before prefix ACK`, async () => {
    const count = mode === 'combined' ? 2 : 1;
    const prefixes = [];
    const local = await server((socket) => {
      socket.once('message', (data) => {
        const start = JSON.parse(data);
        socket.send(
          JSON.stringify(
            accepted(
              start,
              start.methodParameters.inputs.map((input, index) => ({
                channel: index + 1,
                direction: 'input',
                streamToken: `packed-${index}`,
                provisionalRef: input.value.provisionalRef,
                inputHighWater: { sequence: '0', terminal: false },
              })),
            ),
          ),
        );
        socket.on('message', (data, binary) => {
          assert.equal(binary, true, 'an unadmitted terminal must not be sent');
          const length = data.readUInt32BE(0);
          prefixes.push([...data.subarray(4 + length)]);
        });
      });
    });
    const controller = new AbortController();
    let timer;
    try {
      const inputs = Array.from({ length: count }, () => {
        let pulls = 0;
        return agentStream({
          [Symbol.asyncIterator]() {
            return {
              next() {
                pulls += 1;
                if (pulls === 1) return Promise.resolve({ done: false, value: 7 });
                const error = new Error(
                  mode === 'combined' ? 'é'.repeat((9 * MiB) / 2) : '\0'.repeat(3 * MiB),
                );
                if (mode === 'synchronous') throw error;
                return waitFor(() => prefixes.length === count).then(() => {
                  throw error;
                });
              },
            };
          },
        });
      });
      const method = createStreamingRemoteMethod(
        () => ({ server: local.endpoint }),
        () => descriptor,
        ([inputs], register) => ({
          inputs: inputs.map((input) =>
            register(input, (value) => ({ kind: 'u8', value }), 'u8', 'u8'),
          ),
        }),
        () => undefined,
      );
      const call = method.abortable(controller.signal, inputs);
      await assert.rejects(
        Promise.race([
          call,
          new Promise((_, reject) => {
            timer = setTimeout(
              () => reject(new Error('failure remained uncharged while prefix ACK was withheld')),
              1000,
            );
          }),
        ]),
        {
          code: 'resource-exhausted',
          message:
            mode === 'combined'
              ? 'unacknowledged input limit exceeded'
              : 'input logical value limit exceeded',
        },
      );
      if (mode !== 'synchronous')
        assert.deepEqual(
          prefixes,
          Array.from({ length: count }, () => [7]),
        );
    } finally {
      clearTimeout(timer);
      controller.abort();
      await local.close();
    }
  });
}

function controlledPackedSource() {
  let pulls = 0;
  let returned = 0;
  let rejectPull;
  return {
    source: agentStream({
      [Symbol.asyncIterator]() {
        return {
          next() {
            pulls += 1;
            if (pulls === 1) return Promise.resolve({ done: false, value: 7 });
            return new Promise((_, reject) => {
              rejectPull = reject;
            });
          },
          return: async () => {
            returned += 1;
            return { done: true };
          },
        };
      },
    }),
    reject: (message) => rejectPull(new Error(message)),
    pulls: () => pulls,
    returned: () => returned,
  };
}

async function speculativeInputSession(sources) {
  let socket;
  let idempotencyKey;
  let connections = 0;
  const frames = [];
  const local = await server((connected) => {
    socket = connected;
    connections += 1;
    socket.once('message', (data) => {
      const operation = JSON.parse(data);
      idempotencyKey ??= operation.idempotencyKey;
      const mappings = sources.map((_, index) => ({
        channel: connections === 1 ? index + 1 : index + 101,
        direction: 'input',
        streamToken: `packed-${index}`,
        ...(connections === 1
          ? { provisionalRef: operation.methodParameters.inputs[index].value.provisionalRef }
          : {}),
        inputHighWater: { sequence: '0', terminal: false },
      }));
      socket.send(
        JSON.stringify(
          accepted(
            operation,
            [...mappings, { channel: 200, direction: 'output', streamToken: 'marker' }],
            idempotencyKey,
          ),
        ),
      );
      if (connections === 1)
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: {
              kind: 'value',
              graph: outputStreamGraph,
              value: { kind: 'stream', value: { streamToken: 'marker' } },
            },
          }),
        );
      socket.on('message', (data, binary) => {
        if (binary) {
          const length = data.readUInt32BE(0);
          frames.push({
            ...JSON.parse(data.subarray(4, 4 + length)),
            bytes: [...data.subarray(4 + length)],
          });
        } else frames.push(JSON.parse(data));
      });
    });
  });
  const controller = new AbortController();
  const method = createStreamingRemoteMethod(
    () => ({ server: local.endpoint }),
    () => descriptor,
    ([inputs], register) => ({
      inputs: inputs.map((input) =>
        register(input, (value) => ({ kind: 'u8', value }), 'u8', 'u8'),
      ),
    }),
    (value, stream) =>
      stream(value.value.value.streamToken, (item) => item)[Symbol.asyncIterator](),
  );
  const marker = await method.abortable(
    controller.signal,
    sources.map(({ source }) => source),
  );
  let sequence = 0;
  const send = (frame) => socket.send(JSON.stringify(frame));
  return {
    frames,
    send,
    disconnect: () => socket.terminate(),
    connections: () => connections,
    barrier: async () => {
      send({
        version: 1,
        type: 'outputStreamItem',
        channel: 200,
        sequence: String(sequence),
        cursorToken: `input-marker-${sequence}`,
        mappings: [],
        value: 'marker',
      });
      sequence += 1;
      assert.equal((await marker.next()).value, 'marker');
    },
    close: async () => {
      controller.abort();
      await local.close();
    },
  };
}

for (const stage of ['reserved', 'late']) {
  test(`packed speculative cancellation before ACK releases ${stage} failure ownership`, async () => {
    const sources = [controlledPackedSource(), controlledPackedSource()];
    const session = await speculativeInputSession(sources);
    try {
      await waitFor(() => session.frames.length === 2);
      const diagnostic = 'é'.repeat((9 * MiB) / 2);
      if (stage === 'reserved') {
        sources[0].reject(diagnostic);
        await session.barrier();
      }
      session.send({
        version: 1,
        type: 'stream-end',
        channel: 1,
        direction: 'input',
        sequence: '0',
        outcome: { kind: 'consumer-cancelled' },
      });
      await session.barrier();
      assert.equal(sources[0].returned(), 1);
      if (stage === 'late') {
        sources[0].reject('\0'.repeat(3 * MiB));
        await session.barrier();
      }
      sources[1].reject(diagnostic);
      await session.barrier();
      assert.equal(session.frames.length, 2, 'failure must wait behind its unacknowledged prefix');
      session.send({
        version: 1,
        type: 'inputStreamAck',
        channel: 2,
        highestContiguousSequence: '1',
        terminal: false,
        mappings: [],
      });
      await waitFor(() => session.frames.length === 3);
      assert.deepEqual(session.frames[2], {
        version: 1,
        type: 'stream-end',
        channel: 2,
        direction: 'input',
        sequence: '1',
        outcome: { kind: 'failed', error: { kind: 'interrupted', message: diagnostic } },
      });
      await session.barrier();
      session.send({
        version: 1,
        type: 'inputStreamAck',
        channel: 2,
        highestContiguousSequence: '1',
        terminal: true,
        mappings: [],
      });
      await session.barrier();
      assert.deepEqual(
        sources.map((source) => source.pulls()),
        [2, 2],
      );
    } finally {
      await session.close();
    }
  });
}

test('packed speculative failure reservation survives prefix replay on reconnect without a double charge', async () => {
  const source = controlledPackedSource();
  const session = await speculativeInputSession([source]);
  try {
    await waitFor(() => session.frames.length === 1);
    const diagnostic = 'é'.repeat((9 * MiB) / 2);
    source.reject(diagnostic);
    await session.barrier();
    session.disconnect();
    await waitFor(() => session.connections() === 2 && session.frames.length === 2);
    assert.deepEqual(
      session.frames.map(({ channel, sequence, bytes }) => ({ channel, sequence, bytes })),
      [
        { channel: 1, sequence: '0', bytes: [7] },
        { channel: 101, sequence: '0', bytes: [7] },
      ],
    );
    session.send({
      version: 1,
      type: 'inputStreamAck',
      channel: 101,
      highestContiguousSequence: '1',
      terminal: false,
      mappings: [],
    });
    await waitFor(() => session.frames.length === 3);
    assert.deepEqual(session.frames[2], {
      version: 1,
      type: 'stream-end',
      channel: 101,
      direction: 'input',
      sequence: '1',
      outcome: { kind: 'failed', error: { kind: 'interrupted', message: diagnostic } },
    });
    await session.barrier();
    session.send({
      version: 1,
      type: 'inputStreamAck',
      channel: 101,
      highestContiguousSequence: '1',
      terminal: true,
      mappings: [],
    });
    await session.barrier();
    assert.equal(source.pulls(), 2);
  } finally {
    await session.close();
  }
});

test('packed speculative failure keeps ownership when prefix ACK starts awaiting the pull', async () => {
  const source = controlledPackedSource();
  const session = await speculativeInputSession([source]);
  try {
    await waitFor(() => session.frames.length === 1);
    session.send({
      version: 1,
      type: 'inputStreamAck',
      channel: 1,
      highestContiguousSequence: '1',
      terminal: false,
      mappings: [],
    });
    await session.barrier();
    const diagnostic = 'é'.repeat((9 * MiB) / 2);
    source.reject(diagnostic);
    await waitFor(() => session.frames.length === 2);
    assert.deepEqual(session.frames[1], {
      version: 1,
      type: 'stream-end',
      channel: 1,
      direction: 'input',
      sequence: '1',
      outcome: { kind: 'failed', error: { kind: 'interrupted', message: diagnostic } },
    });
    await session.barrier();
    assert.equal(source.pulls(), 2);
  } finally {
    await session.close();
  }
});

test('session cleanup invalidates a speculative pull before its late rejection', async () => {
  const source = controlledPackedSource();
  const session = await speculativeInputSession([source]);
  await waitFor(() => session.frames.length === 1);
  await session.close();
  source.reject('\0'.repeat(3 * MiB));
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(source.returned(), 1);
  assert.equal(source.pulls(), 2);
  assert.equal(session.frames.length, 1);
});

for (const terminal of [
  { direction: 'input', outcome: { kind: 'finished' }, code: 'invalid-channel' },
  { direction: 'input', outcome: { kind: 'consumer-cancelled' }, code: 'invalid-channel' },
  { direction: 'invalid', outcome: { kind: 'finished' }, code: 'malformed-message' },
  { direction: 'output', outcome: { kind: 'future-terminal' }, code: 'malformed-message' },
  {
    direction: 'output',
    outcome: { kind: 'failed', error: { kind: 'future-error' } },
    code: 'malformed-message',
  },
]) {
  test(`invalid output terminal fails rather than EOF: ${JSON.stringify(terminal)}`, async () => {
    const local = await server((socket) => {
      socket.once('message', (data) => {
        const start = JSON.parse(data);
        socket.send(JSON.stringify(accepted(start, [outputMapping])));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: {
              kind: 'value',
              graph: outputStreamGraph,
              value: { kind: 'stream', value: { streamToken: 'output' } },
            },
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'stream-end',
            channel: 2,
            sequence: '0',
            direction: terminal.direction,
            outcome: terminal.outcome,
          }),
        );
      });
    });
    try {
      const method = createStreamingRemoteMethod(
        () => ({ server: local.endpoint }),
        () => descriptor,
        () => ({}),
        (value, stream) => stream(value.value.value.streamToken, (item) => item),
      );
      const output = await method();
      await assert.rejects(output[Symbol.asyncIterator]().next(), { code: terminal.code });
    } finally {
      await local.close();
    }
  });
}

test('strict WebSocket JSON parsing retains collection and depth budgets', () => {
  assert.throws(() => parseStreamSessionTextFrame(`[${'null,'.repeat(100_000)}null]`), {
    code: 'malformed-message',
  });
  assert.doesNotThrow(() =>
    parseStreamSessionTextFrame(
      JSON.stringify({
        kind: 'list',
        value: {
          elements: Array.from({ length: 40_000 }, (_, value) => ({
            kind: 'u32',
            value,
          })),
        },
      }),
    ),
  );
  assert.throws(() => parseStreamSessionTextFrame(`${'['.repeat(64)}null${']'.repeat(64)}`), {
    code: 'malformed-message',
  });
});

test('AgentStream rejects a second iterator deterministically', async () => {
  const stream = agentStream(
    (async function* () {
      yield 1;
    })(),
  );
  assert.equal((await stream[Symbol.asyncIterator]().next()).value, 1);
  assert.throws(() => stream[Symbol.asyncIterator](), /only be iterated once/);
});

const TEST_ACCOUNT_ID = '00000000-0000-0000-0000-0000000000a1';
const OTHER_ACCOUNT_ID = '00000000-0000-0000-0000-0000000000a2';
const TEST_ENVIRONMENT_ID = '00000000-0000-0000-0000-0000000000e1';
const OTHER_ENVIRONMENT_ID = '00000000-0000-0000-0000-0000000000e2';
const TEST_AGENT_ID = '00000007-0000-8000-8000-000000000004';
const TEST_FINGERPRINT = '00000000-0000-4000-8000-00000000f001';
const TEST_AGENT_URL = `/v1/runtime/envs/${TEST_ENVIRONMENT_ID}/agent-types/agent`;

/**
 * Answers the environment resolution requests of the bridge: a token of `TEST_ACCOUNT_ID`, which
 * sees the named environment in its own account and in another one.
 */
function environmentResponse(url) {
  const { pathname, searchParams } = new URL(url, 'http://localhost');
  if (pathname === '/v1/me/token')
    return Response.json({
      id: '00000000-0000-0000-0000-0000000000t1',
      accountId: TEST_ACCOUNT_ID,
      createdAt: '2026-01-01T00:00:00Z',
      expiresAt: '2027-01-01T00:00:00Z',
    });
  if (pathname === '/v1/me/visible-environments') {
    const environment = (id, accountId) => ({
      environment: { id, name: searchParams.get('env_name') },
      application: { name: searchParams.get('app_name') },
      account: { id: accountId, name: 'account', email: 'account@example.test' },
    });
    return Response.json({
      values: [
        environment(OTHER_ENVIRONMENT_ID, OTHER_ACCOUNT_ID),
        environment(TEST_ENVIRONMENT_ID, TEST_ACCOUNT_ID),
      ],
    });
  }
  return undefined;
}

/** A `fetch` that answers environment resolution and passes every other request to `handler`. */
function restFetch(handler) {
  return async (url, init) => environmentResponse(url) ?? handler(url, init);
}

/** Answers the environment resolution of a Node HTTP server; returns whether it did. */
async function answerEnvironment(request, response) {
  const answer = environmentResponse(request.url);
  if (answer === undefined) return false;
  response.writeHead(answer.status, { 'content-type': 'application/json' });
  response.end(await answer.text());
  return true;
}

async function withFetch(handler, run) {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = restFetch(handler);
  try {
    return await run();
  } finally {
    globalThis.fetch = originalFetch;
  }
}

const restServer = { type: 'custom', url: 'http://example.test', token: 'test' };
const agentKey = {
  appName: 'app',
  envName: 'env',
  agentTypeName: 'agent',
  parameters: { kind: 'record', value: { fields: [] } },
};
const invocation = {
  appName: 'app',
  envName: 'env',
  agentTypeName: 'agent',
  parameters: { kind: 'record', value: { fields: [] } },
  methodName: 'run',
  methodParameters: { kind: 'record', value: { fields: [] } },
  mode: 'await',
};
const invocationResult = (result) => ({
  agentId: TEST_AGENT_ID,
  idempotencyKey: '00000000-0000-4000-8000-000000000001',
  componentRevision: 1,
  result,
});

test('REST request JSON preserves canonical string-form u64 values', async () => {
  let body = '';
  let path = '';
  const http = createHttpServer(async (request, response) => {
    if (await answerEnvironment(request, response)) return;
    path = request.url;
    request.setEncoding('utf8');
    request.on('data', (chunk) => (body += chunk));
    request.on('end', () => {
      response.writeHead(201, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ agentId: TEST_AGENT_ID, agentFingerprint: TEST_FINGERPRINT }));
    });
  });
  http.listen(0, '127.0.0.1');
  await once(http, 'listening');
  try {
    const reference = await createAgent(
      { type: 'custom', url: `http://127.0.0.1:${http.address().port}`, token: 'test' },
      { ...agentKey, parameters: { kind: 'u64', value: '18446744073709551615' } },
    );
    assert.equal(path, `${TEST_AGENT_URL}/agents`);
    assert.match(body, /"value":"18446744073709551615"/u);
    assert.deepEqual(reference, { agentId: TEST_AGENT_ID, agentFingerprint: TEST_FINGERPRINT });
  } finally {
    await new Promise((resolve) => http.close(resolve));
  }
});

test('REST request JSON preserves native f32 and f64 negative zero values', async () => {
  const bodies = [];
  await withFetch(
    async (_url, init) => {
      bodies.push(init.body);
      return Response.json(
        bodies.length === 1
          ? { agentId: TEST_AGENT_ID, agentFingerprint: TEST_FINGERPRINT }
          : invocationResult({
              kind: 'value',
              graph: { root: { kind: 'record', value: { fields: [] } } },
              value: { kind: 'record', value: { fields: [] } },
            }),
      );
    },
    async () => {
      await createAgent(restServer, {
        ...agentKey,
        parameters: { kind: 'f32', value: -0 },
        config: [{ path: ['float-shaped'], value: { kind: 'f32', value: -0 } }],
      });
      await invokeAgent(restServer, {
        ...invocation,
        methodParameters: {
          kind: 'record',
          value: {
            fields: [
              { kind: 'f64', value: -0 },
              { kind: 'u32', value: -0 },
            ],
          },
        },
      });
    },
  );
  assert.match(bodies[0], /"parameters":\{"kind":"f32","value":-0\}/u);
  assert.match(
    bodies[0],
    /"config":\[\{"path":\["float-shaped"\],"value":\{"kind":"f32","value":0\}\}\]/u,
  );
  assert.match(
    bodies[1],
    /"methodParameters":\{"kind":"record","value":\{"fields":\[\{"kind":"f64","value":-0\},\{"kind":"u32","value":0\}\]\}\}/u,
  );
});

test('createAgent posts keyed, config and env to the agents of the agent type', async () => {
  const requests = [];
  await withFetch(
    async (url, init) => {
      requests.push({ url, method: init.method, body: JSON.parse(init.body) });
      return Response.json(
        { agentId: TEST_AGENT_ID, agentFingerprint: TEST_FINGERPRINT },
        { status: 201 },
      );
    },
    async () => {
      const keyed = await createAgent(restServer, {
        ...agentKey,
        keyed: true,
        config: [{ path: ['greeting'], value: { kind: 'string', value: 'hello' } }],
        env: { MODE: 'test' },
      });
      assert.deepEqual(keyed, { agentId: TEST_AGENT_ID, agentFingerprint: TEST_FINGERPRINT });
      await createAgent(restServer, agentKey);
    },
  );
  assert.deepEqual(requests, [
    {
      url: `http://example.test${TEST_AGENT_URL}/agents`,
      method: 'POST',
      body: {
        parameters: agentKey.parameters,
        config: [{ path: ['greeting'], value: { kind: 'string', value: 'hello' } }],
        env: { MODE: 'test' },
        keyed: true,
      },
    },
    {
      url: `http://example.test${TEST_AGENT_URL}/agents`,
      method: 'POST',
      body: { parameters: agentKey.parameters, config: [], env: {}, keyed: false },
    },
  ]);
});

test('createAgent reports a keyed conflict as a GolemConflictError', async () => {
  await withFetch(
    async () =>
      Response.json(
        { code: 'conflict', error: 'the keyed agent exists' },
        { status: 409, statusText: 'Conflict' },
      ),
    async () => {
      const error = await createAgent(restServer, { ...agentKey, keyed: true }).then(
        () => assert.fail('a conflicting keyed create succeeded'),
        (error) => error,
      );
      assert.ok(error instanceof GolemConflictError);
      assert.ok(error instanceof GolemServiceError);
      assert.equal(error._tag, 'GolemConflictError');
      assert.equal(error.operation, 'createAgent');
      assert.equal(error.status, 409);
      assert.deepEqual(error.body, { code: 'conflict', error: 'the keyed agent exists' });
      assert.match(error.message, /^Agent creation failed: 409 Conflict/u);
    },
  );
});

test('createAgent does not classify an idempotency conflict as a keyed conflict', async () => {
  await withFetch(
    async () => Response.json({ code: 'idempotency-conflict' }, { status: 422 }),
    async () => {
      await assert.rejects(createAgent(restServer, { ...agentKey, keyed: true }), (error) => {
        assert.ok(error instanceof GolemServiceError);
        assert.ok(!(error instanceof GolemConflictError));
        assert.equal(error.status, 422);
        return true;
      });
    },
  );
});

test('upsertAgent and lookupAgent post the constructor parameters of the key', async () => {
  const requests = [];
  await withFetch(
    async (url, init) => {
      requests.push({ url, body: JSON.parse(init.body) });
      if (url.endsWith('/lookup') && requests.length > 2)
        return Response.json({ code: 'not-found', error: 'no agent' }, { status: 404 });
      return Response.json({ agentId: TEST_AGENT_ID, agentFingerprint: TEST_FINGERPRINT });
    },
    async () => {
      const reference = { agentId: TEST_AGENT_ID, agentFingerprint: TEST_FINGERPRINT };
      assert.deepEqual(await upsertAgent(restServer, agentKey), reference);
      assert.deepEqual(await lookupAgent(restServer, agentKey), reference);
      assert.equal(await lookupAgent(restServer, agentKey), undefined);
    },
  );
  assert.deepEqual(
    requests.map(({ url }) => url),
    [
      `http://example.test${TEST_AGENT_URL}/agents/upsert`,
      `http://example.test${TEST_AGENT_URL}/agents/lookup`,
      `http://example.test${TEST_AGENT_URL}/agents/lookup`,
    ],
  );
  for (const { body } of requests) assert.deepEqual(body, { parameters: agentKey.parameters });
});

const agentType = { appName: 'app', envName: 'env', agentTypeName: 'agent' };
const TEST_AGENT_METADATA_URL = `http://example.test/v1/runtime/envs/${TEST_ENVIRONMENT_ID}/agents/${TEST_AGENT_ID}`;

/** Agent metadata as the agent metadata endpoint returns it, reduced to the fields the bridge reads. */
const agentMetadata = (agentTypeName) => ({
  agentId: TEST_AGENT_ID,
  ownerKind: 'agent',
  agentTypeName,
  environmentId: TEST_ENVIRONMENT_ID,
  status: 'Idle',
  fingerprint: TEST_FINGERPRINT,
});

test('getAgent returns the ID and fingerprint of an agent of the agent type', async () => {
  const requests = [];
  await withFetch(
    async (url, init) => {
      requests.push({ url, method: init.method ?? 'GET', body: init.body });
      return Response.json(agentMetadata('agent'));
    },
    async () => {
      assert.deepEqual(await getAgent(restServer, agentType, TEST_AGENT_ID), {
        agentId: TEST_AGENT_ID,
        agentFingerprint: TEST_FINGERPRINT,
      });
    },
  );
  assert.deepEqual(requests, [{ url: TEST_AGENT_METADATA_URL, method: 'GET', body: undefined }]);
});

test('getAgent reports an agent of another agent type as not found', async () => {
  await withFetch(
    async () => Response.json(agentMetadata('other')),
    async () => {
      const error = await getAgent(restServer, agentType, TEST_AGENT_ID).then(
        () => assert.fail('an agent of another type was returned'),
        (error) => error,
      );
      assert.equal(error.name, 'GolemServiceError');
      assert.equal(error.operation, 'getAgent');
      assert.equal(error.status, 404);
      assert.equal(error.body.code, 'AGENT_NOT_FOUND');
      assert.match(error.message, /^Agent retrieval failed: 404 Not Found/u);
      assert.match(error.message, /is not of the agent type 'agent'/u);
    },
  );
});

test('getAgent reports an unknown agent as not found', async () => {
  await withFetch(
    async () =>
      Response.json(
        { code: 'AGENT_NOT_FOUND', error: 'agent not found' },
        { status: 404, statusText: 'Not Found' },
      ),
    async () => {
      const error = await getAgent(restServer, agentType, TEST_AGENT_ID).then(
        () => assert.fail('an unknown agent was returned'),
        (error) => error,
      );
      assert.equal(error.name, 'GolemServiceError');
      assert.equal(error.operation, 'getAgent');
      assert.equal(error.status, 404);
      assert.deepEqual(error.body, { code: 'AGENT_NOT_FOUND', error: 'agent not found' });
    },
  );
});

test('invokeAgent addresses the agent by exactly one of agentId or parameters', async () => {
  const requests = [];
  await withFetch(
    async (url, init) => {
      requests.push({ url, headers: init.headers, body: JSON.parse(init.body) });
      return Response.json(invocationResult(undefined));
    },
    async () => {
      const byId = await invokeAgent(restServer, {
        ...invocation,
        parameters: undefined,
        agentId: TEST_AGENT_ID,
        expectedFingerprint: TEST_FINGERPRINT,
        mode: 'schedule',
        scheduleAt: '2026-01-01T00:00:00Z',
        idempotencyKey: 'retry-key',
      });
      assert.equal(byId.agentId, TEST_AGENT_ID);
      await invokeAgent(restServer, invocation);
      for (const request of [
        { ...invocation, agentId: TEST_AGENT_ID },
        { ...invocation, parameters: undefined },
      ])
        await assert.rejects(invokeAgent(restServer, request), {
          name: 'TypeError',
          message: /exactly one of agentId or parameters/u,
        });
    },
  );
  assert.equal(requests.length, 2);
  assert.equal(requests[0].url, `http://example.test${TEST_AGENT_URL}/invoke`);
  assert.equal(requests[0].headers['Idempotency-Key'], 'retry-key');
  assert.deepEqual(requests[0].body, {
    agentId: TEST_AGENT_ID,
    method: 'run',
    methodParameters: invocation.methodParameters,
    mode: 'schedule',
    scheduleAt: '2026-01-01T00:00:00Z',
    expectedFingerprint: TEST_FINGERPRINT,
  });
  assert.equal(requests[1].headers['Idempotency-Key'], undefined);
  assert.deepEqual(requests[1].body, {
    parameters: invocation.parameters,
    method: 'run',
    methodParameters: invocation.methodParameters,
    mode: 'await',
  });
});

test('the environment resolves in the account of the token and is cached', async () => {
  const resolutions = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    const answer = environmentResponse(url);
    if (answer !== undefined) {
      resolutions.push(url);
      return answer;
    }
    return Response.json({ agentId: TEST_AGENT_ID, agentFingerprint: TEST_FINGERPRINT });
  };
  const server = { type: 'custom', url: 'http://cache.example.test', token: 'cache' };
  try {
    await upsertAgent(server, agentKey);
    await upsertAgent(server, { ...agentKey, agentTypeName: 'other' });
  } finally {
    globalThis.fetch = originalFetch;
  }
  assert.deepEqual(resolutions, [
    'http://cache.example.test/v1/me/token',
    'http://cache.example.test/v1/me/visible-environments?app_name=app&env_name=env',
  ]);
});

test('an environment outside the account of the token does not resolve', async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    const { pathname } = new URL(url);
    if (pathname === '/v1/me/token')
      return Response.json({
        id: 'token',
        accountId: TEST_ACCOUNT_ID,
        createdAt: '',
        expiresAt: '',
      });
    if (pathname === '/v1/me/visible-environments')
      return Response.json({
        values: [
          {
            environment: { id: OTHER_ENVIRONMENT_ID, name: 'env' },
            application: { name: 'app' },
            account: { id: OTHER_ACCOUNT_ID, name: 'other', email: 'other@example.test' },
          },
        ],
      });
    return assert.fail(`unexpected request ${url}`);
  };
  try {
    await assert.rejects(
      upsertAgent({ type: 'custom', url: 'http://shared.example.test', token: 'shared' }, agentKey),
      /Environment 'env' of application 'app' does not exist in the account of the token/u,
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test('invokeAgent losslessly parses nested 64-bit schema integers', async () => {
  await withFetch(
    async () =>
      new Response(`{
      "agentId": "${TEST_AGENT_ID}",
      "idempotencyKey": "key",
      "componentRevision": 7,
      "result": {
        "graph": ${JSON.stringify(integerRecordGraph)},
        "value": { "value": { "fields": [
          { "value": "18446744073709551615", "kind": "u64" },
          { "kind": "s64", "value": "-9223372036854775808" },
          { "kind": "tuple", "value": { "elements": [
            { "value": "42", "kind": "s64" },
            { "kind": "record", "value": { "fields": [
              { "kind": "u64", "value": "9" },
              { "kind": "f64", "value": 3.5 },
              { "kind": "string", "value": "escaped: {\\\"kind\\\":\\\"u64\\\",\\\"value\\\":18446744073709551615}" }
            ] } }
          ] } }
        ] }, "kind": "record" }
      }
    }`),
    async () => {
      const response = await invokeAgent(restServer, invocation);
      const fields = response.result.value.value.fields;
      assert.equal(fields[0].value, '18446744073709551615');
      assert.equal(fields[1].value, '-9223372036854775808');
      assert.equal(fields[2].value.elements[0].value, '42');
      assert.equal(fields[2].value.elements[1].value.fields[0].value, '9');
      assert.equal(fields[2].value.elements[1].value.fields[1].value, 3.5);
      assert.equal(
        fields[2].value.elements[1].value.fields[2].value,
        'escaped: {"kind":"u64","value":18446744073709551615}',
      );
      assert.equal(response.agentId, TEST_AGENT_ID);
      assert.equal(response.componentRevision, 7);
      assert.equal(response.result.graph.root.kind, 'record');
    },
  );
});

test('invokeAgent preserves canonical string-form 64-bit integers', async () => {
  await withFetch(
    async () =>
      new Response(`{
      "agentId": "${TEST_AGENT_ID}",
      "idempotencyKey": "key",
      "componentRevision": 1,
      "result": { "kind": "value", "graph": ${JSON.stringify(numericFormsGraph)}, "value": { "kind": "record", "value": { "fields": [
        { "kind": "u64", "value": "1000" },
        { "kind": "s64", "value": "-200" },
        { "kind": "u64", "value": "18446744073709551615" },
        { "kind": "s64", "value": "-9223372036854775808" },
        { "kind": "u64", "value": "9007199254740993" },
        { "kind": "f64", "value": -0 }
      ] } } }
    }`),
    async () => {
      const response = await invokeAgent(restServer, invocation);
      const fields = response.result.value.value.fields;
      assert.equal(fields[0].value, '1000');
      assert.equal(fields[1].value, '-200');
      assert.equal(fields[2].value, '18446744073709551615');
      assert.equal(fields[3].value, '-9223372036854775808');
      assert.equal(fields[4].value, '9007199254740993');
      assert.equal(Object.is(fields[5].value, -0), true);
    },
  );
});

test('invokeAgent rejects nonintegral and out-of-range 64-bit numeric forms', async () => {
  for (const [kind, numericForm, expected] of [
    ['s64', '1e-1', /not an integer/u],
    ['u64', '1.5', /not an integer/u],
    ['s64', '9223372036854775808', /out of range/u],
    ['u64', '18446744073709551616e0', /out of range/u],
    ['u64', '1e1000000000000000', /out of range/u],
    ['s64', '1e-1000000000000000', /not an integer/u],
  ]) {
    await withFetch(
      async () =>
        new Response(`{
          "agentId": "${TEST_AGENT_ID}",
          "idempotencyKey": "key",
          "componentRevision": 1,
          "result": { "kind": "value", "graph": { "root": { "kind": "${kind}", "value": {} } }, "value": { "kind": "${kind}", "value": ${numericForm} } }
        }`),
      () => assert.rejects(invokeAgent(restServer, invocation), expected, `${kind} ${numericForm}`),
    );
  }
});

test('invokeAgent accepts REST results exceeding WebSocket collection budgets', async () => {
  const elements = Array.from({ length: 40_000 }, (_, value) => ({ kind: 'u32', value }));
  await withFetch(
    async () =>
      Response.json(
        invocationResult({
          kind: 'value',
          graph: u32ListGraph,
          value: { kind: 'list', value: { elements } },
        }),
      ),
    async () => {
      const response = await invokeAgent(restServer, invocation);
      assert.equal(response.result.value.value.elements.length, 40_000);
      assert.deepEqual(response.result.value.value.elements.at(-1), {
        kind: 'u32',
        value: 39_999,
      });
    },
  );
});

test('invokeAgent accepts REST results exceeding WebSocket depth budgets', async () => {
  let deepType = { kind: 'u32', value: {} };
  for (let depth = 0; depth < 70; depth += 1)
    deepType = { kind: 'list', value: { element: deepType } };
  const graph = {
    defs: [{ id: 'unused.deep', body: deepType }],
    root: { kind: 'u32', value: {} },
  };
  await withFetch(
    async () =>
      Response.json(invocationResult({ kind: 'value', value: { kind: 'u32', value: 1 }, graph })),
    async () => {
      const response = await invokeAgent(restServer, invocation);
      let nested = response.result.graph.defs[0].body;
      for (let depth = 0; depth < 70; depth += 1) nested = nested.value.element;
      assert.equal(nested.kind, 'u32');
    },
  );
});

const descriptor = {
  application: 'app',
  environment: 'env',
  agentType: 'agent',
  target: { kind: 'key', constructorParameters: {} },
  config: [],
  method: 'run',
};

/** An HTTP server that answers environment resolution and upgrades WebSocket requests. */
async function environmentServer(options = {}) {
  const http = createHttpServer(async (request, response) => {
    if (!(await answerEnvironment(request, response))) {
      response.writeHead(404);
      response.end();
    }
  });
  const wss = new WebSocketServer({ server: http, ...options });
  http.listen(0, '127.0.0.1');
  await once(http, 'listening');
  return {
    wss,
    endpoint: { type: 'custom', url: `http://127.0.0.1:${http.address().port}`, token: 'test' },
    close: () => {
      for (const client of wss.clients) client.terminate();
      http.closeAllConnections();
      return new Promise((resolve) => wss.close(() => http.close(resolve)));
    },
  };
}

async function server(handler) {
  const local = await environmentServer({
    handleProtocols: (protocols) => [...protocols][0],
  });
  local.wss.on('connection', handler);
  return local;
}

async function waitFor(predicate, message = 'condition was not reached') {
  for (let i = 0; i < 200; i += 1) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail(message);
}

function accepted(start, mappings = [], idempotencyKey = start.idempotencyKey) {
  return {
    version: 1,
    type: 'invocationAccepted',
    attemptId: start.attemptId,
    agentId: TEST_AGENT_ID,
    idempotencyKey,
    sessionToken: 'session',
    mappings,
  };
}

test('a session opens on the environment of its descriptor and selects the agent by key', async () => {
  let path;
  let start;
  const local = await server((socket, request) => {
    path = request.url;
    socket.once('message', (data) => {
      start = JSON.parse(data);
      finishWith(socket, start, 5);
    });
  });
  try {
    const value = await plainMethod({ server: local.endpoint })();
    assert.equal(value, 5);
    assert.equal(path, `/v1/runtime/envs/${TEST_ENVIRONMENT_ID}/invoke-agent-session`);
    assert.deepEqual(start.selector, {
      application: 'app',
      environment: 'env',
      agentType: 'agent',
      target: { kind: 'key', constructorParameters: {} },
      method: 'run',
    });
  } finally {
    await local.close();
  }
});

test('a session targets an ID and fingerprint without constructor parameters', async () => {
  let start;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      start = JSON.parse(data);
      finishWith(socket, start, 7);
    });
  });
  const target = { kind: 'id', agentId: TEST_AGENT_ID, expectedFingerprint: TEST_FINGERPRINT };
  try {
    const invoke = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => ({ ...descriptor, target }),
      () => ({}),
      (value) => value.value.value,
    );
    assert.equal(await invoke(), 7);
    assert.deepEqual(start.selector.target, target);
    assert.ok(!('constructorParameters' in start.selector));
  } finally {
    await local.close();
  }
});

test('an accepted agent ID that is not a UUID is malformed', async () => {
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(JSON.stringify({ ...accepted(start), agentId: 'agent()' }));
    });
  });
  try {
    await assert.rejects(plainMethod({ server: local.endpoint })(), {
      code: 'malformed-message',
      message: /invalid accepted invocation identity/u,
    });
  } finally {
    await local.close();
  }
});

test('acceptance keeps the final identity separate from a non-UUID request seed', async () => {
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      assert.equal(start.idempotencyKey, 'non-uuid-request-seed');
      socket.send(JSON.stringify(accepted(start, [], '00000000-0000-0000-0000-000000000009')));
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: { kind: 'value', graph: u32Graph, value: { kind: 'u32', value: 42 } },
        }),
      );
      socket.send(
        JSON.stringify({ version: 1, type: 'invocationFinished', outcome: { kind: 'success' } }),
      );
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => ({ ...descriptor, idempotencyKey: 'non-uuid-request-seed' }),
      () => ({}),
      (value) => value.value.value,
    );
    assert.equal(await method(), 42);
  } finally {
    await local.close();
  }
});

test('uncertain pre-acceptance close exact-retries the frozen start', async () => {
  const starts = [];
  const local = await server((socket) => {
    socket.once('message', (data) => {
      starts.push(data.toString());
      if (starts.length === 1) socket.terminate();
      else {
        const start = JSON.parse(data);
        socket.send(JSON.stringify(accepted(start)));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: { kind: 'value', graph: u32Graph, value: { kind: 'u32', value: 42 } },
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: { kind: 'success' },
          }),
        );
      }
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value) => value.value.value,
    );
    assert.equal(await method(), 42);
    assert.equal(starts.length, 2);
    assert.equal(starts[0], starts[1]);
  } finally {
    await local.close();
  }
});

test('packed u8 input uses native nodes and a v1 binary envelope', async () => {
  let binary;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      const provisionalRef = start.methodParameters.input.value.provisionalRef;
      socket.send(JSON.stringify(accepted(start)));
      socket.send(
        JSON.stringify({
          ...accepted(start, [
            {
              channel: 1,
              direction: 'input',
              streamToken: 'input',
              provisionalRef,
              inputHighWater: { sequence: '0', terminal: false },
            },
          ]),
          type: 'invocationReady',
        }),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: unitGraph,
            value: { kind: 'tuple', value: { elements: [] } },
          },
        }),
      );
      socket.once('message', (frame, isBinary) => {
        assert.equal(isBinary, true);
        const length = frame.readUInt32BE(0);
        const metadata = frame.subarray(4, 4 + length).toString();
        binary = {
          metadata,
          payload: [...frame.subarray(4 + length)],
        };
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: { kind: 'success' },
          }),
        );
      });
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      ([input], register) => ({
        input: register(input, (value) => ({ kind: 'u8', value }), 'u8'),
      }),
      () => undefined,
    );
    await method(
      agentStream(
        (async function* () {
          yield 7;
        })(),
      ),
    );
    for (let i = 0; i < 50 && !binary; i += 1) await new Promise((r) => setTimeout(r, 10));
    assert.deepEqual(binary.payload, [7]);
    assert.equal(
      binary.metadata,
      '{"channel":1,"itemCount":"1","kind":"input-u8","sequence":"0","version":1}',
    );
  } finally {
    await local.close();
  }
});

test('binary input uses native record and binary nodes', async () => {
  let received;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      const streamNode = start.methodParameters.value.fields[0];
      const provisionalRef = streamNode.value.provisionalRef;
      socket.send(
        JSON.stringify(
          accepted(start, [
            {
              channel: 1,
              direction: 'input',
              streamToken: 'input',
              provisionalRef,
              inputHighWater: { sequence: '0', terminal: false },
            },
          ]),
        ),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: unitGraph,
            value: { kind: 'tuple', value: { elements: [] } },
          },
        }),
      );
      socket.once('message', (frame, isBinary) => {
        assert.equal(isBinary, true);
        const length = frame.readUInt32BE(0);
        received = {
          metadata: JSON.parse(frame.subarray(4, 4 + length)),
          payload: [...frame.subarray(4 + length)],
        };
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: { kind: 'success' },
          }),
        );
      });
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      ([input], register) => ({
        kind: 'record',
        value: {
          fields: [
            register(
              input,
              ({ bytes, mimeType }) => ({ kind: 'binary', value: { bytes, mimeType } }),
              'binary',
            ),
          ],
        },
      }),
      () => undefined,
    );
    await method(
      agentStream(
        (async function* () {
          yield { bytes: [0, 255], mimeType: 'application/octet-stream' };
        })(),
      ),
    );
    await waitFor(() => received !== undefined);
    assert.deepEqual(received, {
      metadata: {
        version: 1,
        kind: 'input-binary',
        channel: 1,
        sequence: '0',
        itemCount: '1',
        mimeType: 'application/octet-stream',
      },
      payload: [0, 255],
    });
  } finally {
    await local.close();
  }
});

test('input ACK high-water replays the unaccepted range without pulling ahead', async () => {
  const frames = [];
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      const provisionalRef = start.methodParameters.input.value.provisionalRef;
      socket.send(
        JSON.stringify(
          accepted(start, [
            {
              channel: 1,
              direction: 'input',
              streamToken: 'input',
              provisionalRef,
              inputHighWater: { sequence: '0', terminal: false },
            },
          ]),
        ),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: unitGraph,
            value: { kind: 'tuple', value: { elements: [] } },
          },
        }),
      );
      socket.on('message', (frame) => {
        const message = JSON.parse(frame);
        frames.push(message);
        if (frames.length === 1) {
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'inputStreamAck',
              channel: 1,
              highestContiguousSequence: '0',
              mappings: [],
              terminal: false,
            }),
          );
          setTimeout(() => {
            assert.equal(frames.length, 1);
            socket.send(
              JSON.stringify({
                version: 1,
                type: 'inputStreamAck',
                channel: 1,
                highestContiguousSequence: '1',
                mappings: [],
                terminal: false,
              }),
            );
          }, 10);
        } else {
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'inputStreamAck',
              channel: 1,
              highestContiguousSequence: '1',
              mappings: [],
              terminal: true,
            }),
          );
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'invocationFinished',
              outcome: { kind: 'success' },
            }),
          );
        }
      });
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      ([input], register) => ({ input: register(input, (value) => value) }),
      () => undefined,
    );
    await method(
      agentStream(
        (async function* () {
          yield 'only';
        })(),
      ),
    );
    for (let i = 0; i < 100 && frames.length < 2; i += 1)
      await new Promise((r) => setTimeout(r, 5));
    assert.equal(frames.length, 2);
    assert.equal(frames[1].type, 'stream-end');
    assert.equal(frames[1].direction, 'input');
    assert.deepEqual(frames[1].outcome, { kind: 'finished' });
    assert.equal(frames[1].sequence, '1');
  } finally {
    await local.close();
  }
});

test('resume remaps and trims a partially accepted packed u8 input batch', async () => {
  let connection = 0;
  let idempotencyKey;
  let provisionalRef;
  let replay;
  const local = await server((socket) => {
    connection += 1;
    socket.once('message', (data) => {
      const operation = JSON.parse(data);
      if (connection === 1) {
        idempotencyKey = operation.idempotencyKey;
        provisionalRef = operation.methodParameters.input.value.provisionalRef;
        socket.send(
          JSON.stringify(
            accepted(operation, [
              {
                channel: 1,
                direction: 'input',
                streamToken: 'input',
                provisionalRef,
                inputHighWater: { sequence: '0', terminal: false },
              },
            ]),
          ),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: {
              kind: 'value',
              graph: unitGraph,
              value: { kind: 'tuple', value: { elements: [] } },
            },
          }),
        );
        socket.once('message', (_frame, isBinary) => {
          assert.equal(isBinary, true);
          socket.terminate();
        });
      } else {
        socket.send(
          JSON.stringify(
            accepted(
              operation,
              [
                {
                  channel: 9,
                  direction: 'input',
                  streamToken: 'input',
                  inputHighWater: { sequence: '1', terminal: false },
                },
              ],
              idempotencyKey,
            ),
          ),
        );
        socket.once('message', (frame, isBinary) => {
          assert.equal(isBinary, true);
          const length = frame.readUInt32BE(0);
          replay = {
            metadata: JSON.parse(frame.subarray(4, 4 + length)),
            payload: [...frame.subarray(4 + length)],
          };
          socket.once('message', (end) => {
            const terminal = JSON.parse(end);
            assert.equal(terminal.type, 'stream-end');
            assert.equal(terminal.direction, 'input');
            assert.deepEqual(terminal.outcome, { kind: 'finished' });
            socket.send(
              JSON.stringify({
                version: 1,
                type: 'inputStreamAck',
                channel: 9,
                highestContiguousSequence: '3',
                mappings: [],
                terminal: true,
              }),
            );
            socket.send(
              JSON.stringify({
                version: 1,
                type: 'invocationFinished',
                outcome: { kind: 'success' },
              }),
            );
          });
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'inputStreamAck',
              channel: 9,
              highestContiguousSequence: '3',
              mappings: [],
              terminal: false,
            }),
          );
        });
      }
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      ([input], register) => ({
        input: register(input, (value) => ({ kind: 'u8', value }), 'u8'),
      }),
      () => undefined,
    );
    await method(
      agentStream(
        (async function* () {
          yield 1;
          yield 2;
          yield 3;
        })(),
      ),
    );
    for (let i = 0; i < 100 && !replay; i += 1) await new Promise((r) => setTimeout(r, 5));
    assert.deepEqual(replay, {
      metadata: {
        version: 1,
        kind: 'input-u8',
        channel: 9,
        sequence: '1',
        itemCount: '2',
      },
      payload: [2, 3],
    });
  } finally {
    await local.close();
  }
});

test('resume checkpoints output only after language-level delivery', async () => {
  let connection = 0;
  let resume;
  let idempotencyKey;
  const local = await server((socket) => {
    connection += 1;
    socket.once('message', (data) => {
      const first = JSON.parse(data);
      if (connection === 1) {
        idempotencyKey = first.idempotencyKey;
        socket.send(JSON.stringify(accepted(first, [outputMapping])));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: {
              kind: 'value',
              graph: outputStreamGraph,
              value: { kind: 'stream', value: { streamToken: 'output' } },
            },
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'outputStreamItem',
            channel: 2,
            sequence: '0',
            cursorToken: 'cursor-0',
            mappings: [],
            value: 'first',
          }),
          () => socket.terminate(),
        );
      } else {
        resume = first;
        socket.send(JSON.stringify(accepted(first, [outputMapping], idempotencyKey)));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'outputStreamItem',
            channel: 2,
            sequence: '0',
            cursorToken: 'cursor-0',
            mappings: [],
            value: 'first',
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'stream-end',
            direction: 'output',
            channel: 2,
            sequence: '1',
            cursorToken: 'cursor-end',
            outcome: { kind: 'finished' },
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: { kind: 'success' },
          }),
        );
      }
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.value.value.streamToken, (item) => item, 'string'),
    );
    const output = await method();
    for (let i = 0; i < 100 && !resume; i += 1) await new Promise((r) => setTimeout(r, 5));
    assert.deepEqual(resume.outputCursors, []);
    const iterator = output[Symbol.asyncIterator]();
    assert.equal((await iterator.next()).value, 'first');
    assert.deepEqual(await iterator.next(), { done: true, value: undefined });
  } finally {
    await local.close();
  }
});

test('packed u8 cursor advances only after the final byte is delivered', async () => {
  let secondAttach;
  let connection = 0;
  let firstSocket;
  let idempotencyKey;
  const local = await server((socket) => {
    connection += 1;
    socket.once('message', (data) => {
      const first = JSON.parse(data);
      if (connection === 1) {
        idempotencyKey = first.idempotencyKey;
        firstSocket = socket;
        socket.send(JSON.stringify(accepted(first, [outputMapping])));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: {
              kind: 'value',
              graph: outputStreamGraph,
              value: { kind: 'stream', value: { streamToken: 'output' } },
            },
          }),
        );
        socket.send(
          encodeStreamSessionBinaryEnvelope(
            {
              version: 1,
              kind: 'output-u8',
              channel: 2,
              sequence: '0',
              itemCount: '3',
              cursorToken: 'packed-cursor',
            },
            [1, 2, 3],
          ),
        );
      } else {
        secondAttach = first;
        socket.send(JSON.stringify(accepted(first, [outputMapping], idempotencyKey)));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'stream-end',
            direction: 'output',
            channel: 2,
            sequence: '3',
            cursorToken: 'packed-end',
            outcome: { kind: 'finished' },
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: { kind: 'success' },
          }),
        );
      }
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.value.value.streamToken, (item) => item.value, 'u8', 'u8'),
    );
    const output = await method();
    const iterator = output[Symbol.asyncIterator]();
    assert.equal((await iterator.next()).value, 1);
    assert.equal((await iterator.next()).value, 2);
    assert.equal((await iterator.next()).value, 3);
    firstSocket.terminate();
    await waitFor(() => secondAttach !== undefined);
    assert.deepEqual(secondAttach.outputCursors, ['packed-cursor']);
  } finally {
    await local.close();
  }
});

test('malformed server message is rejected with a stable protocol error', async () => {
  const local = await server((socket) => {
    socket.once('message', () =>
      socket.send('{"version":1,"type":"invocationAccepted","type":"invocationRejected"}'),
    );
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value) => value,
    );
    await assert.rejects(method(), (error) => error.code === 'malformed-message');
  } finally {
    await local.close();
  }
});

test('streaming invocation rejects a value inconsistent with its returned graph', async () => {
  let locallyDecoded = false;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(JSON.stringify(accepted(start)));
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: u32Graph,
            value: { kind: 'string', value: 'not a u32' },
          },
        }),
      );
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({ kind: 'record', value: { fields: [] } }),
      () => {
        locallyDecoded = true;
        return 0;
      },
    );
    await assert.rejects(method(), { code: 'validation-error' });
    assert.equal(locallyDecoded, false);
  } finally {
    await local.close();
  }
});

test('strict JSON rejects non-JSON whitespace and unpaired Unicode surrogates', async () => {
  for (const invalid of [
    '{"version":1,"type":"invocationAccepted"}\u00a0',
    '{"version":1,"type":"invocationRejected","attemptId":"\\ud800","code":"rejected","message":"bad"}',
  ]) {
    const local = await server((socket) => {
      socket.once('message', () => socket.send(invalid));
    });
    try {
      const method = createStreamingRemoteMethod(
        () => ({ server: local.endpoint }),
        () => descriptor,
        () => ({}),
        (value) => value,
      );
      await assert.rejects(method(), (error) => error.code === 'malformed-message');
    } finally {
      await local.close();
    }
  }
});

function codec(body) {
  return publicValueCodec({ defs: new Map(), root: schemaType(body) });
}

test('public value codec enforces canonical values, restrictions, and strict shapes', () => {
  const u64 = codec({
    tag: 'u64',
    restrictions: {
      min: { tag: 'unsigned', val: 9_007_199_254_740_993n },
      max: { tag: 'unsigned', val: 18_446_744_073_709_551_615n },
    },
  });
  assert.deepEqual(u64.validate({ kind: 'u64', value: '18446744073709551615' }, 'none'), {
    kind: 'u64',
    value: '18446744073709551615',
  });
  assert.throws(() => u64.validate(9_007_199_254_740_994, 'none'), {
    code: 'validation-error',
  });
  assert.throws(() => u64.validate('09007199254740993', 'none'), {
    code: 'validation-error',
  });
  assert.throws(() => codec({ tag: 'string' }).validate('\ud800', 'none'), {
    code: 'validation-error',
  });

  const record = codec({
    tag: 'record',
    fields: [
      {
        name: 'enabled',
        body: schemaType({ tag: 'bool' }),
        metadata: { aliases: [], examples: [] },
      },
    ],
  });
  const recordValue = {
    kind: 'record',
    value: { fields: [{ kind: 'bool', value: true }] },
  };
  assert.deepEqual(record.validate(recordValue, 'none'), recordValue);
  assert.throws(() => record.validate({ enabled: true }, 'none'), {
    code: 'validation-error',
  });

  const binary = codec({
    tag: 'binary',
    restrictions: {
      minBytes: 2,
      maxBytes: 2,
      mimeTypes: ['application/octet-stream'],
    },
  });
  assert.deepEqual(
    binary.validate(
      { kind: 'binary', value: { bytes: [251, 255], mimeType: 'application/octet-stream' } },
      'none',
    ),
    { kind: 'binary', value: { bytes: [251, 255], mimeType: 'application/octet-stream' } },
  );
  assert.throws(() => binary.validate({ kind: 'binary', value: { bytes: [256] } }, 'none'), {
    code: 'validation-error',
  });
  // Only the server checks restrictions; the bridge checks the wire form.
  const outsideRestrictions = {
    kind: 'binary',
    value: { bytes: [1, 2, 3], mimeType: 'image/png' },
  };
  assert.deepEqual(binary.validate(outsideRestrictions, 'none'), outsideRestrictions);
  assert.throws(() => binary.validate({ bytes: '+/8=', mimeType: 'image/png' }, 'none'), {
    code: 'validation-error',
  });

  const url = codec({ tag: 'url', restrictions: { allowedSchemes: ['mailto'] } });
  assert.deepEqual(
    url.validate({ kind: 'url', value: { url: 'mailto:test@example.com' } }, 'none'),
    { kind: 'url', value: { url: 'mailto:test@example.com' } },
  );

  const quantity = codec({
    tag: 'quantity',
    spec: {
      baseUnit: 'kg',
      allowedSuffixes: ['kg'],
      min: { mantissa: 9_007_199_254_740_993n, scale: 0, unit: 'kg' },
      max: { mantissa: 9_007_199_254_740_995n, scale: 0, unit: 'kg' },
    },
  });
  assert.deepEqual(
    quantity.validate(
      { kind: 'quantity', value: { mantissa: '9007199254740994', scale: 0, unit: 'kg' } },
      'none',
    ),
    { kind: 'quantity', value: { mantissa: '9007199254740994', scale: 0, unit: 'kg' } },
  );
  assert.throws(
    () => quantity.validate({ mantissa: '9007199254740992', scale: 0, unit: 'kg' }, 'none'),
    { code: 'validation-error' },
  );
});

test('public value codec enforces union discriminators and affine stream policy', () => {
  const union = codec({
    tag: 'union',
    branches: [
      {
        tag: 'event',
        body: schemaType({
          tag: 'record',
          fields: [
            {
              name: 'kind',
              body: schemaType({ tag: 'string' }),
              metadata: { aliases: [], examples: [] },
            },
          ],
        }),
        discriminator: {
          tag: 'field-equals',
          val: { fieldName: 'kind', literal: 'event' },
        },
        metadata: { aliases: [], examples: [] },
      },
    ],
  });
  const event = {
    kind: 'union',
    value: {
      tag: 'event',
      body: { kind: 'record', value: { fields: [{ kind: 'string', value: 'event' }] } },
    },
  };
  assert.deepEqual(union.validate(event, 'none'), event);
  assert.throws(
    () =>
      union.validate(
        {
          kind: 'union',
          value: {
            tag: 'event',
            body: { kind: 'record', value: { fields: [{ kind: 'string', value: 'other' }] } },
          },
        },
        'none',
      ),
    {
      code: 'validation-error',
    },
  );

  const streams = codec({
    tag: 'list',
    element: schemaType({ tag: 'stream', element: schemaType({ tag: 'u8' }) }),
  });
  const reference = {
    kind: 'stream',
    value: { provisionalRef: '0dff1c71-f12f-4bb1-996c-23d693bdc825' },
  };
  const one = { kind: 'list', value: { elements: [reference] } };
  assert.deepEqual(streams.validate(one, 'provisional'), one);
  assert.throws(() => streams.validate(one, 'none'), { code: 'unsupported-value' });
  assert.throws(
    () =>
      streams.validate(
        { kind: 'list', value: { elements: [reference, reference] } },
        'provisional',
      ),
    {
      code: 'stream-already-consumed',
    },
  );
  assert.throws(
    () =>
      streams.validate(
        {
          kind: 'list',
          value: { elements: [{ kind: 'stream', value: { streamToken: 'stable' } }] },
        },
        'provisional',
      ),
    {
      code: 'validation-error',
    },
  );
  assert.throws(() => codec({ tag: 'future' }).validate(null, 'none'), {
    code: 'unsupported-value',
  });
});

test('wire schema graphs validate their own values before local decoding', () => {
  const graph = schemaGraphFromWire({
    defs: [
      {
        id: 'result',
        body: {
          kind: 'record',
          value: {
            fields: [{ name: 'count', body: { kind: 'u32', value: {} } }],
          },
        },
      },
    ],
    root: { kind: 'ref', value: { id: 'result' } },
  });
  assert.doesNotThrow(() =>
    publicValueCodec(graph).validate(
      { kind: 'record', value: { fields: [{ kind: 'u32', value: 7 }] } },
      'stable',
    ),
  );
  assert.throws(
    () =>
      publicValueCodec(graph).validate(
        { kind: 'record', value: { fields: [{ kind: 'string', value: 'wrong' }] } },
        'stable',
      ),
    { code: 'validation-error' },
  );
  assert.throws(
    () => schemaGraphFromWire({ root: { kind: 'ref', value: { id: 'missing' } } }),
    /unresolved schema reference/u,
  );
});

test('definitive WebSocket handshake failure does not retry', async () => {
  let attempts = 0;
  const local = await environmentServer({
    verifyClient: (_info, done) => {
      attempts += 1;
      done(false, 401, 'Unauthorized');
    },
  });
  const endpoint = local.endpoint;
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: endpoint }),
      () => descriptor,
      () => ({}),
      (value) => value,
    );
    await assert.rejects(method(), { code: 'handshake-failed' });
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(attempts, 1);
  } finally {
    await local.close();
  }
});

test('normal close before invocation finish reconnects with a fresh resume', async () => {
  const operations = [];
  let idempotencyKey;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const operation = JSON.parse(data);
      operations.push(operation);
      if (operations.length === 1) {
        idempotencyKey = operation.idempotencyKey;
        socket.send(JSON.stringify(accepted(operation)));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: { kind: 'value', graph: u32Graph, value: { kind: 'u32', value: 42 } },
          }),
          () => socket.close(1000),
        );
      } else {
        socket.send(JSON.stringify(accepted(operation, [], idempotencyKey)));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: { kind: 'success' },
          }),
        );
      }
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value) => value.value.value,
    );
    assert.equal(await method(), 42);
    await waitFor(() => operations.length === 2);
    assert.equal(operations[1].type, 'resumeAttach');
    assert.notEqual(operations[1].attemptId, operations[0].attemptId);
  } finally {
    await local.close();
  }
});

test('uncertain resume close exact-retries the frozen resume descriptor', async () => {
  const operations = [];
  let idempotencyKey;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const text = data.toString();
      const operation = JSON.parse(text);
      operations.push(text);
      if (operations.length === 1) {
        idempotencyKey = operation.idempotencyKey;
        socket.send(JSON.stringify(accepted(operation)));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: { kind: 'value', graph: u32Graph, value: { kind: 'u32', value: 7 } },
          }),
          () => socket.terminate(),
        );
      } else if (operations.length === 2) socket.terminate();
      else {
        socket.send(JSON.stringify(accepted(operation, [], idempotencyKey)));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: { kind: 'success' },
          }),
        );
      }
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value) => value.value.value,
    );
    assert.equal(await method(), 7);
    await waitFor(() => operations.length === 3);
    assert.equal(operations[1], operations[2]);
    assert.equal(JSON.parse(operations[1]).type, 'resumeAttach');
  } finally {
    await local.close();
  }
});

for (const secondRejection of [undefined, false, true]) {
  test(`resume attachment rejection takes over once (second rejection: ${secondRejection})`, async () => {
    const operations = [];
    let firstSocket;
    let idempotencyKey;
    const local = await server((socket) => {
      socket.once('message', (data) => {
        const text = data.toString();
        const operation = JSON.parse(text);
        operations.push(text);
        if (operations.length === 1) {
          firstSocket = socket;
          idempotencyKey = operation.idempotencyKey;
          socket.send(JSON.stringify(accepted(operation, [outputMapping])));
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'invocationResult',
              mappings: [],
              result: {
                kind: 'value',
                graph: outputStreamGraph,
                value: { kind: 'stream', value: { streamToken: 'output' } },
              },
            }),
          );
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'outputStreamItem',
              channel: 2,
              sequence: '0',
              cursorToken: 'delivered-first',
              mappings: [],
              value: 'first',
            }),
          );
        } else if (
          operations.length === 2 ||
          (operations.length === 4 && secondRejection !== undefined)
        ) {
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'invocationRejected',
              attemptId: operation.attemptId,
              code: 'invalid-attachment-state',
              message: 'old attachment remains',
              retryable: operations.length === 2 ? false : secondRejection,
            }),
          );
        } else if (operations.length === 3) {
          socket.terminate();
        } else {
          socket.send(
            JSON.stringify(accepted(operation, [{ ...outputMapping, channel: 9 }], idempotencyKey)),
          );
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'outputStreamItem',
              channel: 9,
              sequence: '1',
              cursorToken: 'delivered-second',
              mappings: [],
              value: 'second',
            }),
          );
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'stream-end',
              direction: 'output',
              channel: 9,
              sequence: '2',
              cursorToken: 'delivered-end',
              outcome: { kind: 'finished' },
            }),
          );
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'invocationFinished',
              outcome: { kind: 'success' },
            }),
          );
        }
      });
    });
    try {
      const method = createStreamingRemoteMethod(
        () => ({ server: local.endpoint }),
        () => descriptor,
        () => ({}),
        (value, stream) => stream(value.value.value.streamToken, (item) => item, 'string'),
      );
      const iterator = (await method())[Symbol.asyncIterator]();
      assert.equal((await iterator.next()).value, 'first');
      firstSocket.terminate();
      if (secondRejection === undefined) {
        assert.equal((await iterator.next()).value, 'second');
        assert.equal((await iterator.next()).done, true);
      } else {
        await assert.rejects(iterator.next(), /old attachment remains/);
      }
      assert.equal(operations.length, 4);
      const resume = JSON.parse(operations[1]);
      const takeover = JSON.parse(operations[2]);
      assert.equal(resume.operation, 'resume');
      assert.deepEqual(resume.outputCursors, ['delivered-first']);
      assert.equal(takeover.operation, 'takeover');
      assert.notEqual(takeover.attemptId, resume.attemptId);
      assert.equal(takeover.sessionToken, resume.sessionToken);
      assert.deepEqual(takeover.outputCursors, resume.outputCursors);
      assert.equal(operations[3], operations[2]);
    } finally {
      await local.close();
    }
  });
}

test('retryable rejection after acceptance creates a fresh resume attempt', async () => {
  const operations = [];
  let idempotencyKey;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const operation = JSON.parse(data);
      operations.push(operation);
      if (operations.length === 1) {
        idempotencyKey = operation.idempotencyKey;
        socket.send(JSON.stringify(accepted(operation)));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: { kind: 'value', graph: u32Graph, value: { kind: 'u32', value: 9 } },
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationRejected',
            attemptId: operation.attemptId,
            code: 'stale-session',
            message: 'retry',
            retryable: true,
          }),
        );
      } else {
        socket.send(JSON.stringify(accepted(operation, [], idempotencyKey)));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: { kind: 'success' },
          }),
        );
      }
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value) => value.value.value,
    );
    assert.equal(await method(), 9);
    await waitFor(() => operations.length === 2);
    assert.equal(operations[1].type, 'resumeAttach');
    assert.notEqual(operations[1].attemptId, operations[0].attemptId);
  } finally {
    await local.close();
  }
});

test('mapping installation rejects duplicate channels without accepting the session', async () => {
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(
        JSON.stringify(
          accepted(start, [
            { channel: 1, direction: 'output', streamToken: 'first' },
            { channel: 1, direction: 'output', streamToken: 'second' },
          ]),
        ),
      );
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value) => value,
    );
    await assert.rejects(method(), { code: 'stream-conflict' });
  } finally {
    await local.close();
  }
});

for (const outcome of [
  {
    kind: 'failed',
    error: { kind: 'interrupted', message: 'input item 1 does not satisfy the element schema' },
  },
  { kind: 'consumer-cancelled', details: 'consumer-drop' },
]) {
  test(`a resumed client gets the input outcome that the server selected: ${outcome.kind}`, async () => {
    let connection = 0;
    let idempotencyKey;
    let returned = 0;
    const local = await server((socket) => {
      connection += 1;
      socket.once('message', (data) => {
        const operation = JSON.parse(data);
        if (connection === 1) {
          idempotencyKey = operation.idempotencyKey;
          const provisionalRef = operation.methodParameters.input.value.provisionalRef;
          socket.send(
            JSON.stringify(
              accepted(operation, [
                {
                  channel: 1,
                  direction: 'input',
                  streamToken: 'input',
                  provisionalRef,
                  inputHighWater: { sequence: '0', terminal: false },
                },
              ]),
            ),
          );
          let items = 0;
          socket.on('message', (message) => {
            if (JSON.parse(message).type !== 'inputStreamItem') return;
            items += 1;
            if (items === 1)
              socket.send(
                JSON.stringify({
                  version: 1,
                  type: 'inputStreamAck',
                  channel: 1,
                  highestContiguousSequence: '1',
                  mappings: [],
                  terminal: false,
                }),
              );
            // The server ends the input at item 1, and the connection is lost
            // before its stream-end reaches the client.
            else socket.terminate();
          });
          return;
        }
        assert.equal(operation.type, 'resumeAttach');
        socket.send(
          JSON.stringify(
            accepted(
              operation,
              [
                {
                  channel: 9,
                  direction: 'input',
                  streamToken: 'input',
                  inputHighWater: { sequence: '1', terminal: true, outcome },
                },
              ],
              idempotencyKey,
            ),
          ),
        );
        if (outcome.kind !== 'failed') {
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'invocationResult',
              mappings: [],
              result: { kind: 'none' },
            }),
          );
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'invocationFinished',
              outcome: { kind: 'success' },
            }),
          );
        }
      });
    });
    try {
      let pulls = 0;
      const source = agentStream({
        [Symbol.asyncIterator]() {
          return {
            next: async () => {
              pulls += 1;
              if (pulls <= 2) return { done: false, value: { kind: 'u32', value: pulls } };
              return new Promise(() => {});
            },
            return: async () => {
              returned += 1;
              return { done: true };
            },
          };
        },
      });
      const method = createStreamingRemoteMethod(
        () => ({ server: local.endpoint }),
        () => descriptor,
        ([input], register) => ({ input: register(input, (value) => value) }),
        () => undefined,
      );
      if (outcome.kind === 'failed')
        await assert.rejects(method(source), (error) => {
          assert.ok(error instanceof StreamingProtocolError);
          assert.equal(error.code, 'interrupted');
          assert.match(error.message, /item 1: input item 1 does not satisfy the element schema/u);
          return true;
        });
      else await method(source);
      assert.equal(connection, 2);
      assert.ok(returned >= 1);
    } finally {
      await local.close();
    }
  });
}

test('input source failure terminal is replayed on the remapped channel after reconnect', async () => {
  const cancellations = [];
  let connection = 0;
  let pulls = 0;
  let idempotencyKey;
  const local = await server((socket) => {
    connection += 1;
    socket.once('message', (data) => {
      const operation = JSON.parse(data);
      if (connection === 1) {
        idempotencyKey = operation.idempotencyKey;
        const provisionalRef = operation.methodParameters.input.value.provisionalRef;
        socket.send(
          JSON.stringify(
            accepted(operation, [
              {
                channel: 1,
                direction: 'input',
                streamToken: 'input',
                provisionalRef,
                inputHighWater: { sequence: '0', terminal: false },
              },
            ]),
          ),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: {
              kind: 'value',
              graph: unitGraph,
              value: { kind: 'tuple', value: { elements: [] } },
            },
          }),
        );
      } else {
        socket.send(
          JSON.stringify(
            accepted(
              operation,
              [
                {
                  channel: 9,
                  direction: 'input',
                  streamToken: 'input',
                  inputHighWater: { sequence: '1', terminal: false },
                },
              ],
              idempotencyKey,
            ),
          ),
        );
      }
      socket.on('message', (message) => {
        const parsed = JSON.parse(message);
        if (parsed.type === 'inputStreamItem') {
          assert.equal(parsed.sequence, '0');
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'inputStreamAck',
              channel: 1,
              highestContiguousSequence: '1',
              mappings: [],
              terminal: false,
            }),
          );
          return;
        }
        if (parsed.type !== 'stream-end') return;
        cancellations.push(parsed);
        if (connection === 1) socket.terminate();
        else {
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'inputStreamAck',
              channel: 9,
              highestContiguousSequence: '1',
              mappings: [],
              terminal: true,
            }),
          );
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'invocationFinished',
              outcome: { kind: 'success' },
            }),
          );
        }
      });
    });
  });
  try {
    const source = agentStream({
      [Symbol.asyncIterator]() {
        return {
          next: async () => {
            pulls += 1;
            if (pulls === 1) return { done: false, value: { kind: 'u32', value: 7 } };
            throw new Error('source failed');
          },
        };
      },
    });
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      ([input], register) => ({ input: register(input, (value) => value) }),
      () => undefined,
    );
    await method(source);
    await waitFor(() => cancellations.length === 2);
    assert.deepEqual(
      cancellations.map(({ channel, direction, sequence, outcome }) => ({
        channel,
        direction,
        sequence,
        outcome,
      })),
      [
        {
          channel: 1,
          direction: 'input',
          sequence: '1',
          outcome: { kind: 'failed', error: { kind: 'interrupted', message: 'source failed' } },
        },
        {
          channel: 9,
          direction: 'input',
          sequence: '1',
          outcome: { kind: 'failed', error: { kind: 'interrupted', message: 'source failed' } },
        },
      ],
    );
    assert.equal(pulls, 2);
  } finally {
    await local.close();
  }
});

test('packed u8 source failure follows already-read bytes with a sequenced interrupted terminal', async () => {
  const frames = [];
  let pulls = 0;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(
        JSON.stringify(
          accepted(start, [
            {
              channel: 1,
              direction: 'input',
              streamToken: 'input',
              provisionalRef: start.methodParameters.input.value.provisionalRef,
              inputHighWater: { sequence: '0', terminal: false },
            },
          ]),
        ),
      );
      socket.on('message', (data, binary) => {
        if (binary) {
          const length = data.readUInt32BE(0);
          const metadata = JSON.parse(data.subarray(4, 4 + length));
          frames.push({ metadata, bytes: [...data.subarray(4 + length)] });
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'inputStreamAck',
              channel: 1,
              highestContiguousSequence: '2',
              terminal: false,
              mappings: [],
            }),
          );
        } else {
          frames.push(JSON.parse(data));
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'inputStreamAck',
              channel: 1,
              highestContiguousSequence: '2',
              terminal: true,
              mappings: [],
            }),
          );
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'invocationResult',
              mappings: [],
              result: { kind: 'none' },
            }),
          );
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'invocationFinished',
              outcome: { kind: 'success' },
            }),
          );
        }
      });
    });
  });
  try {
    const source = agentStream({
      [Symbol.asyncIterator]() {
        return {
          next: async () => {
            pulls += 1;
            if (pulls <= 2) return { done: false, value: pulls };
            throw new StreamingProtocolError('local-code', 'packed source failed');
          },
        };
      },
    });
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      ([input], register) => ({
        input: register(input, (value) => ({ kind: 'u8', value }), 'u8', 'u8'),
      }),
      () => undefined,
    );
    await method(source);
    assert.equal(pulls, 3);
    assert.deepEqual(frames[0].bytes, [1, 2]);
    assert.equal(frames[0].metadata.sequence, '0');
    assert.deepEqual(frames[1], {
      version: 1,
      type: 'stream-end',
      direction: 'input',
      channel: 1,
      sequence: '2',
      outcome: { kind: 'failed', error: { kind: 'interrupted', message: 'packed source failed' } },
    });
  } finally {
    await local.close();
  }
});

for (const outcome of [
  { kind: 'consumer-cancelled', details: null },
  { kind: 'failed', error: { kind: 'trapped', message: 'callee trapped' } },
  { kind: 'failed', error: { kind: 'interrupted', message: 'protocol failure' } },
]) {
  test(`server input terminal stops an in-flight source pull without a client terminal: ${JSON.stringify(outcome)}`, async () => {
    let returned = 0;
    const frames = [];
    let resolvePull;
    const local = await server((socket) => {
      socket.once('message', (data) => {
        const start = JSON.parse(data);
        socket.send(
          JSON.stringify(
            accepted(start, [
              {
                channel: 1,
                direction: 'input',
                streamToken: 'input',
                provisionalRef: start.methodParameters.input.value.provisionalRef,
                inputHighWater: { sequence: '0', terminal: false },
              },
            ]),
          ),
        );
        socket.on('message', (data) => frames.push(JSON.parse(data)));
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'stream-end',
            direction: 'input',
            channel: 1,
            sequence: '0',
            outcome,
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: { kind: 'none' },
          }),
        );
        socket.send(
          JSON.stringify({ version: 1, type: 'invocationFinished', outcome: { kind: 'success' } }),
        );
      });
    });
    try {
      const source = agentStream({
        [Symbol.asyncIterator]() {
          return {
            next: () =>
              new Promise((resolve) => {
                resolvePull = resolve;
              }),
            return: async () => {
              returned += 1;
              resolvePull({ done: true });
              return { done: true };
            },
          };
        },
      });
      const method = createStreamingRemoteMethod(
        () => ({ server: local.endpoint }),
        () => descriptor,
        ([input], register) => ({ input: register(input, (value) => value) }),
        () => undefined,
      );
      if (outcome.kind === 'failed')
        // A failed input end is at an item the caller sent, so the call fails with it.
        await assert.rejects(method(source), (error) => {
          assert.ok(error instanceof StreamingProtocolError);
          assert.equal(error.code, outcome.error.kind);
          assert.deepEqual(error.outcome, outcome);
          return true;
        });
      else await method(source);
      await new Promise((resolve) => setTimeout(resolve, 10));
      assert.equal(returned, 1);
      assert.deepEqual(frames, []);
    } finally {
      await local.close();
    }
  });
}

test('an input that the server ends with a failure fails the call with that failure', async () => {
  const local = await server((socket) => {
    let items = 0;
    socket.on('message', (data) => {
      const message = JSON.parse(data);
      if (message.type === 'invocationStart') {
        const provisionalRef = message.methodParameters.input.value.provisionalRef;
        socket.send(
          JSON.stringify(
            accepted(message, [
              {
                channel: 1,
                direction: 'input',
                streamToken: 'input',
                provisionalRef,
                inputHighWater: { sequence: '0', terminal: false },
              },
            ]),
          ),
        );
      } else if (message.type === 'inputStreamItem' && ++items === 1) {
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'inputStreamAck',
            channel: 1,
            highestContiguousSequence: '1',
            mappings: [],
            terminal: false,
          }),
        );
      } else if (message.type === 'inputStreamItem') {
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'stream-end',
            channel: 1,
            direction: 'input',
            sequence: '1',
            outcome: {
              kind: 'failed',
              error: {
                kind: 'interrupted',
                message: 'input item 1 does not satisfy the element schema',
              },
            },
          }),
        );
      }
    });
  });
  try {
    const source = agentStream(
      (async function* () {
        yield { kind: 'u32', value: 1 };
        yield { kind: 'u32', value: 2 };
      })(),
    );
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      ([input], register) => ({ input: register(input, (value) => value) }),
      () => undefined,
    );
    await assert.rejects(method(source), (error) => {
      assert.ok(error instanceof StreamingProtocolError);
      assert.equal(error.code, 'interrupted');
      assert.match(error.message, /item 1: input item 1 does not satisfy the element schema/u);
      return true;
    });
  } finally {
    await local.close();
  }
});

test('consumer drop sends cancellation and remains distinct from protocol terminal', async () => {
  let cancellation;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(JSON.stringify(accepted(start, [outputMapping])));
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: outputStreamGraph,
            value: { kind: 'stream', value: { streamToken: 'output' } },
          },
        }),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'outputStreamItem',
          channel: 2,
          sequence: '0',
          cursorToken: 'first',
          mappings: [],
          value: 'first',
        }),
      );
      socket.on('message', (message) => {
        const parsed = JSON.parse(message);
        if (parsed.type !== 'stream-end') return;
        cancellation = parsed;
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'stream-end',
            direction: 'output',
            channel: 2,
            sequence: '1',
            cursorToken: 'cancelled',
            outcome: { kind: 'consumer-cancelled', details: 'consumer-drop' },
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: { kind: 'success' },
          }),
        );
      });
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.value.value.streamToken, (item) => item, 'string'),
    );
    const output = await method();
    const iterator = output[Symbol.asyncIterator]();
    assert.equal((await iterator.next()).value, 'first');
    assert.deepEqual(await iterator.return(), { done: true, value: undefined });
    await waitFor(() => cancellation !== undefined);
    assert.deepEqual(cancellation, {
      version: 1,
      type: 'stream-end',
      direction: 'output',
      sequence: '0',
      channel: 2,
      outcome: { kind: 'consumer-cancelled', details: 'consumer-drop' },
    });
  } finally {
    await local.close();
  }
});

test('generated output decode failure rejects delivery and terminates without cursor resume', async () => {
  let connections = 0;
  const local = await server((socket) => {
    connections += 1;
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(JSON.stringify(accepted(start, [outputMapping])));
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: outputStreamGraph,
            value: { kind: 'stream', value: { streamToken: 'output' } },
          },
        }),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'outputStreamItem',
          channel: 2,
          sequence: '0',
          cursorToken: 'must-not-resume',
          mappings: [],
          value: 'invalid',
        }),
      );
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value, stream) =>
        stream(value.value.value.streamToken, () => {
          throw new Error('generated decode failed');
        }),
    );
    const output = await method();
    await assert.rejects(output[Symbol.asyncIterator]().next(), /generated decode failed/);
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(connections, 1);
  } finally {
    await local.close();
  }
});

test('packed u8 output larger than the queue item limit is delivered lazily', async () => {
  const payload = Uint8Array.from({ length: 300 }, (_, index) => index % 256);
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(JSON.stringify(accepted(start, [outputMapping])));
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: outputStreamGraph,
            value: { kind: 'stream', value: { streamToken: 'output' } },
          },
        }),
      );
      socket.send(
        encodeStreamSessionBinaryEnvelope(
          {
            version: 1,
            kind: 'output-u8',
            channel: 2,
            sequence: '0',
            itemCount: payload.length.toString(),
            cursorToken: 'large-packed',
          },
          payload,
        ),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'stream-end',
          direction: 'output',
          channel: 2,
          sequence: payload.length.toString(),
          cursorToken: 'large-end',
          outcome: { kind: 'finished' },
        }),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationFinished',
          outcome: { kind: 'success' },
        }),
      );
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.value.value.streamToken, (item) => item.value, 'u8', 'u8'),
    );
    const output = await method();
    const actual = [];
    for await (const byte of output) actual.push(byte);
    assert.deepEqual(actual, [...payload]);
  } finally {
    await local.close();
  }
});

test('binary output passes a native binary node to the generated decoder', async () => {
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(JSON.stringify(accepted(start, [outputMapping])));
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: outputStreamGraph,
            value: { kind: 'stream', value: { streamToken: 'output' } },
          },
        }),
      );
      socket.send(
        encodeStreamSessionBinaryEnvelope(
          {
            version: 1,
            kind: 'output-binary',
            channel: 2,
            sequence: '0',
            itemCount: '1',
            cursorToken: 'binary-cursor',
            mimeType: 'application/octet-stream',
          },
          [0, 255],
        ),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'stream-end',
          direction: 'output',
          channel: 2,
          sequence: '1',
          cursorToken: 'binary-end',
          outcome: { kind: 'finished' },
        }),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationFinished',
          outcome: { kind: 'success' },
        }),
      );
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value, stream) =>
        stream(
          value.value.value.streamToken,
          (item) => {
            assert.deepEqual(item, {
              kind: 'binary',
              value: { bytes: [0, 255], mimeType: 'application/octet-stream' },
            });
            return item.value;
          },
          'binary',
          'binary',
        ),
    );
    const output = await method();
    assert.deepEqual(await output[Symbol.asyncIterator]().next(), {
      done: false,
      value: { bytes: [0, 255], mimeType: 'application/octet-stream' },
    });
  } finally {
    await local.close();
  }
});

test('backend attachment failure resumes open output before authoritative completion', async () => {
  const operations = [];
  let idempotencyKey;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const operation = JSON.parse(data);
      operations.push(operation);
      if (operations.length === 1) idempotencyKey = operation.idempotencyKey;
      socket.send(JSON.stringify(accepted(operation, [outputMapping], idempotencyKey)));
      if (operations.length === 1) {
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: {
              kind: 'value',
              graph: outputStreamGraph,
              value: { kind: 'stream', value: { streamToken: 'output' } },
            },
          }),
          () => socket.close(1011, 'session attachment failed'),
        );
      } else {
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'stream-end',
            direction: 'output',
            channel: 2,
            sequence: '0',
            cursorToken: 'recorded-terminal',
            outcome: {
              kind: 'failed',
              error: { kind: 'trapped', message: 'recorded stream failure' },
            },
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: {
              kind: 'failure',
              code: 'invocation-failed',
              message: 'recorded invocation failure',
            },
          }),
        );
      }
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.value.value.streamToken, (item) => item, 'string'),
    );
    const output = await method();
    const iterator = output[Symbol.asyncIterator]();
    await assert.rejects(iterator.next(), /recorded stream failure/);
    assert.deepEqual(await iterator.next(), { done: true, value: undefined });
    assert.equal(operations.length, 2);
    assert.equal(operations[1].type, 'resumeAttach');
    assert.equal(operations[1].sessionToken, 'session');
    assert.notEqual(operations[1].attemptId, operations[0].attemptId);
  } finally {
    await local.close();
  }
});

test('invocation failure preserves an already-received output terminal', async () => {
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(JSON.stringify(accepted(start, [outputMapping])));
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: outputStreamGraph,
            value: { kind: 'stream', value: { streamToken: 'output' } },
          },
        }),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'stream-end',
          direction: 'output',
          channel: 2,
          sequence: '0',
          cursorToken: 'output-error-cursor',
          outcome: { kind: 'failed', error: { kind: 'trapped', message: 'stream failed first' } },
        }),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationFinished',
          outcome: {
            kind: 'failure',
            code: 'invocation-broke',
            message: 'invocation failed later',
          },
        }),
      );
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.value.value.streamToken, (item) => item, 'string'),
    );
    const output = await method();
    await new Promise((resolve) => setTimeout(resolve, 20));
    const iterator = output[Symbol.asyncIterator]();
    await assert.rejects(iterator.next(), /stream failed first/);
    assert.deepEqual(await iterator.next(), { done: true, value: undefined });
  } finally {
    await local.close();
  }
});

test('concurrent output pulls each settle in stream order', async () => {
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      socket.send(JSON.stringify(accepted(start, [outputMapping])));
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: {
            kind: 'value',
            graph: outputStreamGraph,
            value: { kind: 'stream', value: { streamToken: 'output' } },
          },
        }),
      );
      setTimeout(() => {
        for (const [sequence, value] of ['first', 'second'].entries()) {
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'outputStreamItem',
              channel: 2,
              sequence: sequence.toString(),
              cursorToken: `cursor-${sequence}`,
              mappings: [],
              value,
            }),
          );
        }
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'stream-end',
            direction: 'output',
            channel: 2,
            sequence: '2',
            cursorToken: 'cursor-end',
            outcome: { kind: 'finished' },
          }),
        );
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationFinished',
            outcome: { kind: 'success' },
          }),
        );
      }, 10);
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint }),
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.value.value.streamToken, (item) => item, 'string'),
    );
    const output = await method();
    const iterator = output[Symbol.asyncIterator]();
    const pulls = [iterator.next(), iterator.next()];
    const results = await Promise.race([
      Promise.all(pulls),
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('concurrent output pull did not settle')), 250),
      ),
    ]);
    assert.deepEqual(results, [
      { done: false, value: 'first' },
      { done: false, value: 'second' },
    ]);
  } finally {
    await local.close();
  }
});

/** A WebSocket server whose upgrades first get the given HTTP statuses. */
async function refusingServer(statuses, handler) {
  const wss = new WebSocketServer({
    noServer: true,
    handleProtocols: (protocols) => [...protocols][0],
  });
  wss.on('connection', handler);
  let upgrades = 0;
  const http = createHttpServer(answerEnvironment);
  http.on('upgrade', (request, socket, head) => {
    const status = statuses[upgrades++];
    if (status !== undefined) {
      // A client abort can reset the socket while the refusal is being written.
      socket.once('error', (error) => assert.equal(error.code, 'ECONNRESET'));
      socket.end(`HTTP/1.1 ${status} Refused\r\nContent-Length: 0\r\nConnection: close\r\n\r\n`);
      return;
    }
    wss.handleUpgrade(request, socket, head, (ws) => wss.emit('connection', ws, request));
  });
  http.listen(0, '127.0.0.1');
  await once(http, 'listening');
  return {
    endpoint: { type: 'custom', url: `http://127.0.0.1:${http.address().port}`, token: 'test' },
    upgrades: () => upgrades,
    close: () => {
      for (const client of wss.clients) client.terminate();
      http.closeAllConnections();
      return new Promise((resolve) => http.close(resolve));
    },
  };
}

function finishWith(socket, start, value = 1) {
  socket.send(JSON.stringify(accepted(start)));
  socket.send(
    JSON.stringify({
      version: 1,
      type: 'invocationResult',
      mappings: [],
      result: { kind: 'value', graph: u32Graph, value: { kind: 'u32', value } },
    }),
  );
  socket.send(
    JSON.stringify({ version: 1, type: 'invocationFinished', outcome: { kind: 'success' } }),
  );
}

/** Shortens how long an accepted connection must stay up to count as progress. */

function plainMethod(configuration) {
  return createStreamingRemoteMethod(
    () => configuration,
    () => descriptor,
    () => ({}),
    (value) => value.value.value,
  );
}

test('a start against a closed port fails with StreamingUnreachableError after the window', async () => {
  const tcp = createTcpServer();
  tcp.listen(0, '127.0.0.1');
  await once(tcp, 'listening');
  const port = tcp.address().port;
  await new Promise((resolve) => tcp.close(resolve));
  const started = Date.now();
  const error = await plainMethod({
    server: { type: 'custom', url: `http://127.0.0.1:${port}`, token: 'test' },
    reconnectWindowMs: 300,
  })().then(
    () => assert.fail('connected to a closed port'),
    (error) => error,
  );
  const elapsed = Date.now() - started;
  assert.ok(error instanceof StreamingUnreachableError, String(error));
  assert.equal(error.reconnectWindowMs, 300);
  assert.match(error.endpoint, new RegExp(`^ws://127\\.0\\.0\\.1:${port}/`));
  assert.ok(error.lastCause instanceof Error);
  assert.ok(elapsed >= 290 && elapsed < 3000, `gave up after ${elapsed} ms`);
});

test('an upgrade that never completes is abandoned within the window', async () => {
  const held = [];
  const http = createHttpServer(answerEnvironment);
  http.on('upgrade', (_request, socket) => held.push(socket));
  http.listen(0, '127.0.0.1');
  await once(http, 'listening');
  try {
    const started = Date.now();
    const error = await plainMethod({
      server: { type: 'custom', url: `http://127.0.0.1:${http.address().port}`, token: 'test' },
      reconnectWindowMs: 400,
    })().then(
      () => assert.fail('an upgrade that never completed connected'),
      (error) => error,
    );
    assert.ok(Date.now() - started < 3000);
    assert.ok(error instanceof StreamingUnreachableError, String(error));
    assert.match(error.lastCause.message, /no WebSocket connection within/);
  } finally {
    for (const socket of held) socket.destroy();
    http.closeAllConnections();
    await new Promise((resolve) => http.close(resolve));
  }
});

test('an environment resolution that is never answered is abandoned within the window', async () => {
  const held = [];
  const tcp = createTcpServer((socket) => held.push(socket));
  tcp.listen(0, '127.0.0.1');
  await once(tcp, 'listening');
  try {
    const started = Date.now();
    const error = await plainMethod({
      server: { type: 'custom', url: `http://127.0.0.1:${tcp.address().port}`, token: 'test' },
      reconnectWindowMs: 400,
    })().then(
      () => assert.fail('an unanswered environment resolution connected'),
      (error) => error,
    );
    assert.ok(Date.now() - started < 3000);
    assert.ok(error instanceof StreamingUnreachableError, String(error));
    assert.match(error.lastCause.message, /no environment resolution within/);
  } finally {
    for (const socket of held) socket.destroy();
    await new Promise((resolve) => tcp.close(resolve));
  }
});

/**
 * A session server that answers `path` of the environment resolution with each of `statuses` in
 * turn before it answers normally, and finishes each start with 9.
 */
async function lookupRefusingServer(statuses, path = '/v1/me/token') {
  let lookups = 0;
  const http = createHttpServer(async (request, response) => {
    if (new URL(request.url, 'http://localhost').pathname === path) {
      const status = statuses[lookups++];
      if (status !== undefined) {
        response.writeHead(status, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ code: 'REFUSED', error: `refused with ${status}` }));
        return;
      }
    }
    if (!(await answerEnvironment(request, response))) {
      response.writeHead(404);
      response.end();
    }
  });
  const wss = new WebSocketServer({
    server: http,
    handleProtocols: (protocols) => [...protocols][0],
  });
  wss.on('connection', (socket) => {
    socket.once('message', (data) => finishWith(socket, JSON.parse(data), 9));
  });
  http.listen(0, '127.0.0.1');
  await once(http, 'listening');
  return {
    endpoint: { type: 'custom', url: `http://127.0.0.1:${http.address().port}`, token: 'test' },
    lookups: () => lookups,
    close: () => {
      for (const client of wss.clients) client.terminate();
      http.closeAllConnections();
      return new Promise((resolve) => wss.close(() => http.close(resolve)));
    },
  };
}

test('an environment resolution answered 503 is retried', async () => {
  const local = await lookupRefusingServer([503]);
  try {
    assert.equal(await plainMethod({ server: local.endpoint, reconnectWindowMs: 10_000 })(), 9);
    assert.equal(local.lookups(), 2);
  } finally {
    await local.close();
  }
});

test('an environment resolution answered 429 is retried', async () => {
  const local = await lookupRefusingServer([429], '/v1/me/visible-environments');
  try {
    assert.equal(await plainMethod({ server: local.endpoint, reconnectWindowMs: 10_000 })(), 9);
    assert.equal(local.lookups(), 2);
  } finally {
    await local.close();
  }
});

test('an environment resolution answered 403 fails at once', async () => {
  const local = await lookupRefusingServer([403]);
  try {
    const error = await plainMethod({ server: local.endpoint, reconnectWindowMs: 10_000 })().then(
      () => assert.fail('a forbidden environment resolution connected'),
      (error) => error,
    );
    assert.equal(error.name, 'GolemServiceError');
    assert.equal(error.operation, 'resolveEnvironment');
    assert.equal(error.status, 403);
    assert.equal(local.lookups(), 1);
  } finally {
    await local.close();
  }
});

test('an environment resolution answered 503 past the window is unreachable', async () => {
  const local = await lookupRefusingServer(Array(1000).fill(503));
  try {
    const error = await plainMethod({ server: local.endpoint, reconnectWindowMs: 300 })().then(
      () => assert.fail('connected through a refusing environment resolution'),
      (error) => error,
    );
    assert.ok(error instanceof StreamingUnreachableError, String(error));
    assert.equal(error.lastCause.name, 'GolemServiceError');
    assert.equal(error.lastCause.operation, 'resolveEnvironment');
    assert.equal(error.lastCause.status, 503);
    assert.ok(local.lookups() > 1);
  } finally {
    await local.close();
  }
});

test('429 and 5xx on the upgrade are retried', async () => {
  const local = await refusingServer([503, 429, 502], (socket) => {
    socket.once('message', (data) => finishWith(socket, JSON.parse(data), 7));
  });
  try {
    assert.equal(await plainMethod({ server: local.endpoint, reconnectWindowMs: 10_000 })(), 7);
    assert.equal(local.upgrades(), 4);
  } finally {
    await local.close();
  }
});

test('another upgrade status fails at once', async () => {
  const local = await refusingServer([403], () => assert.fail('upgraded after a refusal'));
  try {
    const error = await plainMethod({ server: local.endpoint })().then(
      () => assert.fail('a refused upgrade connected'),
      (error) => error,
    );
    assert.ok(error instanceof StreamingProtocolError);
    assert.equal(error.code, 'handshake-failed');
    assert.equal(local.upgrades(), 1);
  } finally {
    await local.close();
  }
});

test('retryable upgrade statuses past the window are unreachable', async () => {
  const local = await refusingServer(Array(1000).fill(503), () => assert.fail('upgraded'));
  try {
    const error = await plainMethod({ server: local.endpoint, reconnectWindowMs: 300 })().then(
      () => assert.fail('connected through a refusing server'),
      (error) => error,
    );
    assert.ok(error instanceof StreamingUnreachableError, String(error));
    assert.equal(error.lastCause.code, 'handshake-failed');
    assert.match(error.lastCause.message, /HTTP 503/);
  } finally {
    await local.close();
  }
});

test('an unlimited reconnect window keeps retrying', async () => {
  const local = await refusingServer([503, 503, 503, 503, 503], (socket) => {
    socket.once('message', (data) => finishWith(socket, JSON.parse(data), 3));
  });
  try {
    assert.equal(
      await plainMethod({ server: local.endpoint, reconnectWindowMs: 'unlimited' })(),
      3,
    );
  } finally {
    await local.close();
  }
});

test('an invalid reconnect window is rejected', async () => {
  await assert.rejects(
    plainMethod({ server: { type: 'local' }, reconnectWindowMs: -1 })(),
    (error) => error instanceof StreamingProtocolError && error.code === 'validation-error',
  );
});

test('a retryable rejection before acceptance starts a new attempt', async () => {
  const starts = [];
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      starts.push(start);
      if (starts.length === 1) {
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationRejected',
            attemptId: start.attemptId,
            code: 'routing-miss',
            message: 'moving',
            retryable: true,
          }),
        );
        return;
      }
      const provisionalRef = start.methodParameters.input.value.provisionalRef;
      socket.send(
        JSON.stringify(
          accepted(start, [
            {
              channel: 1,
              direction: 'input',
              streamToken: 'input',
              provisionalRef,
              inputHighWater: { sequence: '0', terminal: false },
            },
          ]),
        ),
      );
      socket.on('message', (frame) => {
        const message = JSON.parse(frame);
        const end = message.type === 'stream-end';
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'inputStreamAck',
            channel: 1,
            highestContiguousSequence: end
              ? message.sequence
              : String(Number(message.sequence) + 1),
            mappings: [],
            terminal: end,
          }),
        );
        if (!end) return;
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: { kind: 'value', graph: u32Graph, value: { kind: 'u32', value: 5 } },
          }),
        );
        socket.send(
          JSON.stringify({ version: 1, type: 'invocationFinished', outcome: { kind: 'success' } }),
        );
      });
    });
  });
  try {
    const method = createStreamingRemoteMethod(
      () => ({ server: local.endpoint, reconnectWindowMs: 10_000 }),
      () => descriptor,
      ([input], register) => ({ input: register(input, (value) => value) }),
      (value) => value.value.value,
    );
    assert.equal(
      await method(
        agentStream(
          (async function* () {
            yield { kind: 'u32', value: 1 };
          })(),
        ),
      ),
      5,
    );
    assert.equal(starts.length, 2);
    assert.notEqual(starts[1].attemptId, starts[0].attemptId);
    assert.equal(starts[1].idempotencyKey, starts[0].idempotencyKey);
    assert.notEqual(
      starts[1].methodParameters.input.value.provisionalRef,
      starts[0].methodParameters.input.value.provisionalRef,
    );
  } finally {
    await local.close();
  }
});

test('retryable rejections past the window are unreachable', async () => {
  const local = await server((socket) => {
    socket.once('message', (data) => {
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationRejected',
          attemptId: JSON.parse(data).attemptId,
          code: 'routing-miss',
          message: 'moving',
          retryable: true,
        }),
      );
    });
  });
  try {
    const error = await plainMethod({ server: local.endpoint, reconnectWindowMs: 400 })().then(
      () => assert.fail('a rejected start succeeded'),
      (error) => error,
    );
    assert.ok(error instanceof StreamingUnreachableError, String(error));
    assert.equal(error.lastCause.code, 'routing-miss');
  } finally {
    await local.close();
  }
});

test('agent failures surface stderr and still reject unknown members', async () => {
  const rejection = (start, extra = {}) => ({
    version: 1,
    type: 'invocationRejected',
    attemptId: start.attemptId,
    code: 'previous-invocation-failed',
    message: 'intentional invocation failure',
    retryable: false,
    stderr: 'rejection stderr\n',
    ...extra,
  });
  const failure = (extra = {}) => ({
    version: 1,
    type: 'invocationFinished',
    outcome: {
      kind: 'failure',
      code: 'invocation-failed',
      message: 'intentional invocation failure',
      stderr: 'failure stderr\n',
      ...extra,
    },
  });
  const cases = [
    {
      reply: (start) => [rejection(start)],
      expected: { code: 'previous-invocation-failed', stderr: 'rejection stderr\n' },
    },
    {
      reply: (start) => [accepted(start), failure()],
      expected: { code: 'invocation-failed', stderr: 'failure stderr\n' },
    },
    {
      reply: (start) => [rejection(start, { stderr: undefined })],
      expected: { code: 'previous-invocation-failed', stderr: undefined },
    },
    {
      reply: (start) => [rejection(start, { extra: 'unknown' })],
      expected: { code: 'malformed-message', stderr: undefined },
    },
    {
      reply: (start) => [accepted(start), failure({ extra: 'unknown' })],
      expected: { code: 'malformed-message', stderr: undefined },
    },
    {
      reply: (start) => [rejection(start, { stderr: 7 })],
      expected: { code: 'malformed-message', stderr: undefined },
    },
  ];
  for (const { reply, expected } of cases) {
    const local = await server((socket) => {
      socket.once('message', (data) => {
        for (const message of reply(JSON.parse(data))) socket.send(JSON.stringify(message));
      });
    });
    try {
      const error = await plainMethod({ server: local.endpoint })().then(
        () => assert.fail('a failed invocation succeeded'),
        (error) => error,
      );
      assert.ok(error instanceof StreamingProtocolError, String(error));
      assert.equal(error.code, expected.code);
      assert.equal(error.stderr, expected.stderr);
      if (expected.stderr !== undefined)
        assert.equal(error.message, 'intentional invocation failure');
    } finally {
      await local.close();
    }
  }
});

test('a start the server never answers is unreachable after the window', async () => {
  let connections = 0;
  const local = await server(() => {
    connections += 1;
  });
  try {
    const started = Date.now();
    const error = await plainMethod({ server: local.endpoint, reconnectWindowMs: 400 })().then(
      () => assert.fail('an unanswered start succeeded'),
      (error) => error,
    );
    const elapsed = Date.now() - started;
    assert.ok(error instanceof StreamingUnreachableError, String(error));
    assert.equal(error.reconnectWindowMs, 400);
    assert.match(error.lastCause.message, /answered neither the start nor the resume/);
    assert.ok(elapsed >= 390 && elapsed < 3000, `gave up after ${elapsed} ms`);
    assert.equal(connections, 1);
  } finally {
    await local.close();
  }
});

test('a server that drops every start is unreachable after the window, with backoff', async () => {
  let connections = 0;
  const local = await server((socket) => {
    connections += 1;
    socket.once('message', () => socket.terminate());
  });
  try {
    const error = await plainMethod({ server: local.endpoint, reconnectWindowMs: 400 })().then(
      () => assert.fail('a dropped start succeeded'),
      (error) => error,
    );
    assert.ok(error instanceof StreamingUnreachableError, String(error));
    assert.ok(connections >= 2 && connections <= 8, `connected ${connections} times`);
  } finally {
    await local.close();
  }
});

test('the reconnect window starts again when an accepted connection is lost', async () => {
  const operations = [];
  let idempotencyKey;
  const local = await server((socket) => {
    socket.once('message', async (data) => {
      const operation = JSON.parse(data);
      operations.push(operation);
      if (operations.length === 1) {
        idempotencyKey = operation.idempotencyKey;
        socket.send(JSON.stringify(accepted(operation)));
        await new Promise((resolve) => setTimeout(resolve, STABLE_CONNECTION_MS + 200));
        socket.terminate();
        return;
      }
      socket.send(JSON.stringify(accepted(operation, [], idempotencyKey)));
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'invocationResult',
          mappings: [],
          result: { kind: 'value', graph: u32Graph, value: { kind: 'u32', value: 8 } },
        }),
      );
      socket.send(
        JSON.stringify({ version: 1, type: 'invocationFinished', outcome: { kind: 'success' } }),
      );
    });
  });
  try {
    assert.equal(
      await plainMethod({
        server: local.endpoint,
        reconnectWindowMs: 300,
      })(),
      8,
    );
    assert.equal(operations[1].type, 'resumeAttach');
  } finally {
    await local.close();
  }
});

test('an accepted connection dropped at once is unreachable after the window, with backoff', async () => {
  const connected = [];
  let idempotencyKey;
  const local = await server((socket) => {
    connected.push(Date.now());
    socket.once('message', (data) => {
      const operation = JSON.parse(data);
      idempotencyKey ??= operation.idempotencyKey;
      socket.send(JSON.stringify(accepted(operation, [], idempotencyKey)));
      socket.terminate();
    });
  });
  try {
    const started = Date.now();
    const error = await plainMethod({
      server: local.endpoint,
      reconnectWindowMs: 400,
    })().then(
      () => assert.fail('a dropped call succeeded'),
      (error) => error,
    );
    const elapsed = Date.now() - started;
    assert.ok(error instanceof StreamingUnreachableError, String(error));
    assert.ok(elapsed >= 390 && elapsed < 3000, `gave up after ${elapsed} ms`);
    assert.ok(
      connected.length >= 3 && connected.length <= 6,
      `connected ${connected.length} times`,
    );
    const lastWait = connected.at(-1) - connected.at(-2);
    assert.ok(lastWait >= 150, `the last reconnect waited only ${lastWait} ms`);
  } finally {
    await local.close();
  }
});

test('stable connections keep a call alive past the reconnect window', async () => {
  const waits = [];
  let lost;
  let idempotencyKey;
  let connections = 0;
  const local = await server((socket) => {
    connections += 1;
    if (lost !== undefined) waits.push(Date.now() - lost);
    const last = connections === 3;
    socket.once('message', async (data) => {
      const operation = JSON.parse(data);
      idempotencyKey ??= operation.idempotencyKey;
      socket.send(JSON.stringify(accepted(operation, [], idempotencyKey)));
      if (last) {
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'invocationResult',
            mappings: [],
            result: { kind: 'value', graph: u32Graph, value: { kind: 'u32', value: 9 } },
          }),
        );
        socket.send(
          JSON.stringify({ version: 1, type: 'invocationFinished', outcome: { kind: 'success' } }),
        );
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, STABLE_CONNECTION_MS + 200));
      lost = Date.now();
      socket.terminate();
    });
  });
  try {
    const started = Date.now();
    assert.equal(
      await plainMethod({
        server: local.endpoint,
        reconnectWindowMs: 400,
      })(),
      9,
    );
    assert.ok(Date.now() - started >= 2 * STABLE_CONNECTION_MS);
    assert.equal(waits.length, 2);
    for (const wait of waits)
      assert.ok(wait < 40, `a reconnect after a stable connection waited ${wait} ms`);
  } finally {
    await local.close();
  }
});

test('aborting a pending connection preserves the abort reason', async () => {
  const controller = new AbortController();
  const reason = new Error('stop the pending connection');
  const held = new Set();
  const tcp = createTcpServer((socket) => {
    held.add(socket);
    controller.abort(reason);
  });
  tcp.listen(0, '127.0.0.1');
  await once(tcp, 'listening');
  try {
    const error = await plainMethod({
      server: { type: 'custom', url: `http://127.0.0.1:${tcp.address().port}`, token: 'test' },
      reconnectWindowMs: 'unlimited',
    })
      .abortable(controller.signal)
      .then(
        () => assert.fail('an aborted connection succeeded'),
        (error) => error,
      );
    assert.equal(error, reason);
  } finally {
    for (const socket of held) socket.destroy();
    await new Promise((resolve) => tcp.close(resolve));
  }
});

test('aborting a start that cannot connect stops its retries', async () => {
  const local = await refusingServer(Array(1000).fill(503), () => assert.fail('upgraded'));
  try {
    const controller = new AbortController();
    const call = plainMethod({ server: local.endpoint, reconnectWindowMs: 'unlimited' })
      .abortable(controller.signal)
      .then(
        () => assert.fail('an aborted call succeeded'),
        (error) => error,
      );
    await waitFor(() => local.upgrades() >= 2);
    controller.abort();
    assert.equal((await call).name, 'AbortError');
    const upgrades = local.upgrades();
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.equal(local.upgrades(), upgrades);
  } finally {
    await local.close();
  }
});

const lookupDescriptor = {
  application: 'app',
  environment: 'env',
  agentType: 'agent',
  target: { kind: 'key', constructorParameters: {} },
  config: [],
  method: 'run',
};

/** Answers both lookup requests with HTTP 200 and the given JSON bodies. */
async function malformedLookupServer(tokenBody, environmentsBody) {
  let lookups = 0;
  const http = createHttpServer((request, response) => {
    const { pathname } = new URL(request.url, 'http://localhost');
    if (pathname === '/v1/me/token') {
      lookups++;
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify(tokenBody));
      return;
    }
    if (pathname === '/v1/me/visible-environments') {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify(environmentsBody));
      return;
    }
    response.writeHead(404);
    response.end();
  });
  http.listen(0, '127.0.0.1');
  await once(http, 'listening');
  return {
    endpoint: {
      type: 'custom',
      url: `http://127.0.0.1:${http.address().port}`,
      token: 'malformed-lookup',
    },
    lookups: () => lookups,
    close: () => {
      http.closeAllConnections();
      return new Promise((resolve) => http.close(resolve));
    },
  };
}

for (const [name, tokenBody, environmentsBody] of [
  ['an environment list without values', { accountId: 'a' }, {}],
  ['an environment entry without an account', { accountId: 'a' }, { values: [{}] }],
]) {
  test(`an environment resolution answered with ${name} fails at once`, async () => {
    const local = await malformedLookupServer(tokenBody, environmentsBody);
    try {
      const error = await createStreamingRemoteMethod(
        () => ({ server: local.endpoint, reconnectWindowMs: 1_500 }),
        () => lookupDescriptor,
        () => ({}),
        (value) => value.value.value,
      )().then(
        () => assert.fail('connected without an environment'),
        (error) => error,
      );
      assert.ok(
        !(error instanceof StreamingUnreachableError),
        `an undecodable lookup answer was retried until the window ended: ${error}`,
      );
      assert.equal(local.lookups(), 1);
    } finally {
      await local.close();
    }
  });
}
