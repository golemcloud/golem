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
  encodeStreamSessionBinaryEnvelope,
  encodeStreamSessionTextFrame,
  parseStreamSessionTextFrame,
  publicValueCodec,
  schemaType,
} from '../dist/index.mjs';

const outputMapping = {
  channel: 2,
  direction: 'output',
  streamToken: 'output',
};

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
    assert.equal(
      encodeStreamSessionTextFrame(parseStreamSessionTextFrame(vector.canonical)),
      vector.canonical,
      vector.name,
    );
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

test('AgentStream rejects a second iterator deterministically', async () => {
  const stream = agentStream(
    (async function* () {
      yield 1;
    })(),
  );
  assert.equal((await stream[Symbol.asyncIterator]().next()).value, 1);
  assert.throws(() => stream[Symbol.asyncIterator](), /only be iterated once/);
});

test('REST request JSON preserves exact bigint values', async () => {
  let body = '';
  const http = createHttpServer((request, response) => {
    request.setEncoding('utf8');
    request.on('data', (chunk) => (body += chunk));
    request.on('end', () => {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(
        JSON.stringify({
          agentId: { componentId: 'component', agentId: 'agent' },
          componentRevision: 1,
        }),
      );
    });
  });
  http.listen(0, '127.0.0.1');
  await once(http, 'listening');
  try {
    await createAgent(
      { type: 'custom', url: `http://127.0.0.1:${http.address().port}`, token: 'test' },
      {
        appName: 'app',
        envName: 'env',
        agentTypeName: 'agent',
        parameters: { kind: 'u64', value: 18_446_744_073_709_551_615n },
      },
    );
    assert.match(body, /"value":18446744073709551615(?:[,}])/u);
    assert.doesNotMatch(body, /"18446744073709551615"/u);
  } finally {
    await new Promise((resolve) => http.close(resolve));
  }
});

const descriptor = {
  application: 'app',
  environment: 'env',
  agentType: 'agent',
  constructorParameters: {},
  config: [],
  method: 'run',
};

async function server(handler) {
  const wss = new WebSocketServer({ port: 0, handleProtocols: (protocols) => [...protocols][0] });
  await once(wss, 'listening');
  const port = wss.address().port;
  wss.on('connection', handler);
  return {
    endpoint: { type: 'custom', url: `http://127.0.0.1:${port}`, token: 'test' },
    close: () => {
      for (const client of wss.clients) client.terminate();
      return new Promise((resolve) => wss.close(resolve));
    },
  };
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
    idempotencyKey,
    sessionToken: 'session',
    mappings,
  };
}

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
            result: { kind: 'value', value: 42 },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value) => value,
    );
    assert.equal(await method(), 42);
    assert.equal(starts.length, 2);
    assert.equal(starts[0], starts[1]);
  } finally {
    await local.close();
  }
});

test('direct binary input uses a v1 binary envelope', async () => {
  let binary;
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      const provisionalRef = start.methodParameters.input.$stream.provisionalRef;
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
          result: { kind: 'none' },
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
      () => local.endpoint,
      () => descriptor,
      ([input], register) => ({ input: register(input, (value) => value, 'u8') }),
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

test('input ACK high-water replays the unaccepted range without pulling ahead', async () => {
  const frames = [];
  const local = await server((socket) => {
    socket.once('message', (data) => {
      const start = JSON.parse(data);
      const provisionalRef = start.methodParameters.input.$stream.provisionalRef;
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
          result: { kind: 'none' },
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
      () => local.endpoint,
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
    assert.equal(frames[1].type, 'inputStreamEnd');
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
        provisionalRef = operation.methodParameters.input.$stream.provisionalRef;
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
            result: { kind: 'none' },
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
            assert.equal(terminal.type, 'inputStreamEnd');
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
      () => local.endpoint,
      () => descriptor,
      ([input], register) => ({ input: register(input, (value) => value, 'u8') }),
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
            result: { kind: 'value', value: { $stream: { streamToken: 'output' } } },
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
            type: 'outputStreamEnd',
            channel: 2,
            sequence: '1',
            cursorToken: 'cursor-end',
            outcome: { kind: 'ok' },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.$stream.streamToken, (item) => item, 'string'),
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
            result: { kind: 'value', value: { $stream: { streamToken: 'output' } } },
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
            type: 'outputStreamEnd',
            channel: 2,
            sequence: '3',
            cursorToken: 'packed-end',
            outcome: { kind: 'ok' },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.$stream.streamToken, (item) => item, 'u8', 'u8'),
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value) => value,
    );
    await assert.rejects(method(), (error) => error.code === 'malformed-message');
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
        () => local.endpoint,
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
  assert.equal(u64.validate('18446744073709551615', 'none'), '18446744073709551615');
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
  assert.deepEqual(record.validate({ enabled: true }, 'none'), { enabled: true });
  assert.throws(() => record.validate({ enabled: true, extra: false }, 'none'), {
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
    binary.validate({ bytes: '+/8=', mimeType: 'application/octet-stream' }, 'none'),
    { bytes: '+/8=', mimeType: 'application/octet-stream' },
  );
  assert.throws(
    () => binary.validate({ bytes: '-_8=', mimeType: 'application/octet-stream' }, 'none'),
    { code: 'malformed-message' },
  );
  assert.throws(() => binary.validate({ bytes: '+/8=', mimeType: 'image/png' }, 'none'), {
    code: 'validation-error',
  });

  const url = codec({ tag: 'url', restrictions: { allowedSchemes: ['mailto'] } });
  assert.equal(url.validate('mailto:test@example.com', 'none'), 'mailto:test@example.com');

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
    quantity.validate({ mantissa: '9007199254740994', scale: 0, unit: 'kg' }, 'none'),
    { mantissa: '9007199254740994', scale: 0, unit: 'kg' },
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
  assert.deepEqual(union.validate({ $union: 'event', value: { kind: 'event' } }, 'none'), {
    $union: 'event',
    value: { kind: 'event' },
  });
  assert.throws(() => union.validate({ $union: 'event', value: { kind: 'other' } }, 'none'), {
    code: 'validation-error',
  });

  const streams = codec({
    tag: 'list',
    element: schemaType({ tag: 'stream', element: schemaType({ tag: 'u8' }) }),
  });
  const reference = {
    $stream: { provisionalRef: '0dff1c71-f12f-4bb1-996c-23d693bdc825' },
  };
  assert.deepEqual(streams.validate([reference], 'provisional'), [reference]);
  assert.throws(() => streams.validate([reference], 'none'), { code: 'unsupported-value' });
  assert.throws(() => streams.validate([reference, reference], 'provisional'), {
    code: 'stream-already-consumed',
  });
  assert.throws(() => streams.validate([{ $stream: { streamToken: 'stable' } }], 'provisional'), {
    code: 'validation-error',
  });
  assert.throws(() => codec({ tag: 'future' }).validate(null, 'none'), {
    code: 'unsupported-value',
  });
});

test('definitive WebSocket handshake failure does not retry', async () => {
  let attempts = 0;
  const wss = new WebSocketServer({
    port: 0,
    verifyClient: (_info, done) => {
      attempts += 1;
      done(false, 401, 'Unauthorized');
    },
  });
  await once(wss, 'listening');
  const endpoint = {
    type: 'custom',
    url: `http://127.0.0.1:${wss.address().port}`,
    token: 'test',
  };
  try {
    const method = createStreamingRemoteMethod(
      () => endpoint,
      () => descriptor,
      () => ({}),
      (value) => value,
    );
    await assert.rejects(method(), { code: 'handshake-failed' });
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(attempts, 1);
  } finally {
    await new Promise((resolve) => wss.close(resolve));
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
            result: { kind: 'value', value: 42 },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value) => value,
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
            result: { kind: 'value', value: 7 },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value) => value,
    );
    assert.equal(await method(), 7);
    await waitFor(() => operations.length === 3);
    assert.equal(operations[1], operations[2]);
    assert.equal(JSON.parse(operations[1]).type, 'resumeAttach');
  } finally {
    await local.close();
  }
});

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
            result: { kind: 'value', value: 9 },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value) => value,
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value) => value,
    );
    await assert.rejects(method(), { code: 'stream-conflict' });
  } finally {
    await local.close();
  }
});

test('input cancellation is replayed on the remapped channel after reconnect', async () => {
  const cancellations = [];
  let connection = 0;
  let idempotencyKey;
  const local = await server((socket) => {
    connection += 1;
    socket.once('message', (data) => {
      const operation = JSON.parse(data);
      if (connection === 1) {
        idempotencyKey = operation.idempotencyKey;
        const provisionalRef = operation.methodParameters.input.$stream.provisionalRef;
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
            result: { kind: 'none' },
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
                  inputHighWater: { sequence: '0', terminal: false },
                },
              ],
              idempotencyKey,
            ),
          ),
        );
      }
      socket.on('message', (message) => {
        const parsed = JSON.parse(message);
        if (parsed.type !== 'streamCancel') return;
        cancellations.push(parsed);
        if (connection === 1) socket.terminate();
        else {
          socket.send(
            JSON.stringify({
              version: 1,
              type: 'inputStreamAck',
              channel: 9,
              highestContiguousSequence: '0',
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
            throw new Error('source failed');
          },
        };
      },
    });
    const method = createStreamingRemoteMethod(
      () => local.endpoint,
      () => descriptor,
      ([input], register) => ({ input: register(input, (value) => value) }),
      () => undefined,
    );
    await method(source);
    await waitFor(() => cancellations.length === 2);
    assert.deepEqual(
      cancellations.map(({ channel, reason }) => ({ channel, reason })),
      [
        { channel: 1, reason: 'source-unavailable' },
        { channel: 9, reason: 'source-unavailable' },
      ],
    );
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
          result: { kind: 'value', value: { $stream: { streamToken: 'output' } } },
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
        if (parsed.type !== 'streamCancel') return;
        cancellation = parsed;
        socket.send(
          JSON.stringify({
            version: 1,
            type: 'outputStreamEnd',
            channel: 2,
            sequence: '1',
            cursorToken: 'cancelled',
            outcome: { kind: 'cancelled', reason: 'consumer-drop' },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.$stream.streamToken, (item) => item, 'string'),
    );
    const output = await method();
    const iterator = output[Symbol.asyncIterator]();
    assert.equal((await iterator.next()).value, 'first');
    assert.deepEqual(await iterator.return(), { done: true, value: undefined });
    await waitFor(() => cancellation !== undefined);
    assert.deepEqual(
      { channel: cancellation.channel, reason: cancellation.reason },
      { channel: 2, reason: 'consumer-drop' },
    );
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
          result: { kind: 'value', value: { $stream: { streamToken: 'output' } } },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value, stream) =>
        stream(value.$stream.streamToken, () => {
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
          result: { kind: 'value', value: { $stream: { streamToken: 'output' } } },
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
          type: 'outputStreamEnd',
          channel: 2,
          sequence: payload.length.toString(),
          cursorToken: 'large-end',
          outcome: { kind: 'ok' },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.$stream.streamToken, (item) => item, 'u8', 'u8'),
    );
    const output = await method();
    const actual = [];
    for await (const byte of output) actual.push(byte);
    assert.deepEqual(actual, [...payload]);
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
          result: { kind: 'value', value: { $stream: { streamToken: 'output' } } },
        }),
      );
      socket.send(
        JSON.stringify({
          version: 1,
          type: 'outputStreamEnd',
          channel: 2,
          sequence: '0',
          cursorToken: 'output-error-cursor',
          outcome: { kind: 'error', code: 'stream-broke', message: 'stream failed first' },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.$stream.streamToken, (item) => item, 'string'),
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
          result: { kind: 'value', value: { $stream: { streamToken: 'output' } } },
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
            type: 'outputStreamEnd',
            channel: 2,
            sequence: '2',
            cursorToken: 'cursor-end',
            outcome: { kind: 'ok' },
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
      () => local.endpoint,
      () => descriptor,
      () => ({}),
      (value, stream) => stream(value.$stream.streamToken, (item) => item, 'string'),
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
