// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  DurableStreamReader,
  DurableStreamWriter,
  type DurableStreamAppendReceipt,
  type DurableStreamAppendRequest,
  type DurableStreamBatch,
  type DurableStreamErrorKind,
  type DurableStreamReadRequest,
} from 'golem:agent/durable-streams@2.0.0';
import type { Secret } from 'golem:core/types@2.0.0';
import {
  createDurableByteWriter,
  createDurableJsonWriter,
  readDurableByteStream,
  readDurableJsonStream,
} from '../src/durableStreams';
import { s } from '../src/schema/markers';
import { agentStreamToHandle, agentStreamFromHandle } from '../src/schema/agentStream';
import { compileSchema } from '../src/schema/adapter';
import { z } from 'zod';
import '../src/schema/zod';

const read = vi.fn<DurableStreamReader['read']>();
const append = vi.fn<DurableStreamWriter['append']>();
const readers = vi.mocked(DurableStreamReader);
const writers = vi.mocked(DurableStreamWriter);
const readerHandles: (DurableStreamReader & { [Symbol.dispose]: ReturnType<typeof vi.fn> })[] = [];
const writerHandles: (DurableStreamWriter & { [Symbol.dispose]: ReturnType<typeof vi.fn> })[] = [];
const url = 'https://streams.example/events';
const encoder = new TextEncoder();
const batch = (
  payload: string | number[],
  patch: Partial<DurableStreamBatch> = {},
): DurableStreamBatch => ({
  payload: typeof payload === 'string' ? encoder.encode(payload) : new Uint8Array(payload),
  contentType: typeof payload === 'string' ? 'application/json' : 'application/octet-stream',
  next: { offset: 'opaque:17', cursor: 'cursor:2' },
  upToDate: false,
  closed: false,
  ...patch,
});
const receipt = (patch: Partial<DurableStreamAppendReceipt> = {}): DurableStreamAppendReceipt => ({
  nextOffset: 'opaque:18',
  epoch: 0n,
  sequence: 0n,
  closed: false,
  ...patch,
});
const failure = (kind: DurableStreamErrorKind, retryAfterMs?: bigint) => ({
  kind,
  message: 'sanitized failure',
  retryAfterMs,
  producerEpoch: undefined,
  expectedSequence: undefined,
});

beforeEach(() => {
  vi.resetAllMocks();
  readerHandles.length = 0;
  writerHandles.length = 0;
  readers.mockImplementation(() => {
    const handle = { read, [Symbol.dispose]: vi.fn() };
    readerHandles.push(handle);
    return handle;
  });
  writers.mockImplementation(() => {
    const handle = { append, [Symbol.dispose]: vi.fn() };
    writerHandles.push(handle);
    return handle;
  });
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe('external Durable Stream readers', () => {
  it('captures descriptors once, isolates handles and releases unused or failed readers', async () => {
    const auth = {} as Secret;
    const options = { url, auth, timeoutMs: 1739 };
    const first = readDurableByteStream(options);
    options.url = 'https://other.example/bytes';
    options.timeoutMs = 23;
    const second = readDurableByteStream(options);
    expect(readers.mock.calls).toEqual([
      [{ url, mode: 'bytes', timeoutMs: 1739n }, auth],
      [{ url: options.url, mode: 'bytes', timeoutMs: 23n }, auth],
    ]);
    expect(readerHandles[0]).not.toBe(readerHandles[1]);
    expect(read).not.toHaveBeenCalled();
    await first.return();
    expect(readerHandles[0][Symbol.dispose]).toHaveBeenCalledTimes(1);
    expect(readerHandles[1][Symbol.dispose]).not.toHaveBeenCalled();
    read.mockRejectedValueOnce(failure('gone'));
    await expect(second.next()).rejects.toMatchObject({ kind: 'gone' });
    expect(readerHandles[1][Symbol.dispose]).not.toHaveBeenCalled();
    await second.return();
    expect(readerHandles[1][Symbol.dispose]).toHaveBeenCalledTimes(1);
    expect(read.mock.contexts).toEqual([readerHandles[1]]);
  });

  it('drains asymmetric buffered items before fetching and yields the final closed payload', async () => {
    const requests: DurableStreamReadRequest[] = [];
    read.mockImplementation(async (request) => {
      requests.push(structuredClone(request));
      return requests.length === 1 ? batch('[11,23,47]') : batch('[83]', { closed: true });
    });
    const stream = readDurableJsonStream(s.u32(), { url });
    expect(read).not.toHaveBeenCalled();
    expect(readers).toHaveBeenCalledExactlyOnceWith(
      { url, mode: 'json', timeoutMs: 30000n },
      undefined,
    );
    for (const value of [11, 23, 47]) expect(await stream.next()).toEqual({ done: false, value });
    expect(read).toHaveBeenCalledTimes(1);
    expect(await stream.next()).toEqual({ done: false, value: 83 });
    expect(await stream.next()).toEqual({ done: true, value: undefined });
    expect(read).toHaveBeenCalledTimes(2);
    expect(read.mock.contexts).toEqual([readerHandles[0], readerHandles[0]]);
    expect(readerHandles[0][Symbol.dispose]).toHaveBeenCalledTimes(1);
    expect(Object.keys(requests[0]).sort()).toEqual(['checkpoint', 'contentType', 'transport']);
    expect(requests.map((r) => r.checkpoint)).toEqual([
      { offset: '-1', cursor: undefined },
      { offset: 'opaque:17', cursor: 'cursor:2' },
    ]);
  });

  it.each(['long-poll', 'sse'] as const)(
    'resolves now once, pins metadata and survives empty %s responses',
    async (live) => {
      const requests: DurableStreamReadRequest[] = [];
      read.mockImplementation(async (request) => {
        requests.push(structuredClone(request));
        if (requests.length === 1) return batch('', { upToDate: true });
        if (requests.length === 2) throw failure('timeout');
        if (requests.length === 3)
          return batch('[]', { upToDate: true, next: { offset: 'opaque:19', cursor: 'cursor:3' } });
        return batch('[71]', { closed: true });
      });
      const stream = readDurableJsonStream(s.u32(), { url, offset: 'now', live });
      const next = stream.next();
      await vi.runAllTimersAsync();
      expect(await next).toEqual({ done: false, value: 71 });
      expect(requests.map((r) => r.transport)).toEqual(['catch-up', live, live, live]);
      expect(requests.map((r) => r.checkpoint.offset)).toEqual([
        'now',
        'opaque:17',
        'opaque:17',
        'opaque:19',
      ]);
      expect(requests.map((r) => r.contentType)).toEqual([
        undefined,
        'application/json',
        'application/json',
        'application/json',
      ]);
      expect(requests[3].checkpoint.cursor).toBe('cursor:3');
      expect(readers).toHaveBeenCalledTimes(1);
      expect(read.mock.contexts.every((handle) => handle === readerHandles[0])).toBe(true);
    },
  );

  it('preserves nested JSON framing, escaping and exact large integer decoder input', async () => {
    read.mockResolvedValueOnce(batch('[[1,7],[3,9]]', { closed: true }));
    const arrays = readDurableJsonStream(z.array(z.number().int()), { url });
    expect((await arrays.next()).value).toEqual([1, 7]);
    expect((await arrays.next()).value).toEqual([3, 9]);
    const text = '["a,]\\\"b", "c\\\\d"]';
    read.mockResolvedValueOnce(batch(text, { closed: true }));
    const strings = readDurableJsonStream(z.string(), { url });
    expect((await strings.next()).value).toBe('a,]"b');
    expect((await strings.next()).value).toBe('c\\d');
    const decode = vi.fn((json: string) => BigInt(json));
    read.mockResolvedValueOnce(batch('[9007199254740993,18446744073709551615]', { closed: true }));
    const integers = readDurableJsonStream(s.u64(), { url, decode });
    expect((await integers.next()).value).toBe(9007199254740993n);
    expect((await integers.next()).value).toBe(18446744073709551615n);
    expect(decode.mock.calls).toEqual([['9007199254740993'], ['18446744073709551615']]);
  });

  it('retains the failed item or malformed batch without another read', async () => {
    read.mockResolvedValueOnce(batch('[17,9007199254740993,29]'));
    const stream = readDurableJsonStream(s.u64(), { url });
    expect((await stream.next()).value).toBe(17n);
    await expect(stream.next()).rejects.toThrow();
    await expect(stream.next()).rejects.toThrow();
    expect(read).toHaveBeenCalledTimes(1);
    read.mockResolvedValueOnce(batch('[broken]'));
    const malformed = readDurableJsonStream(s.u32(), { url });
    await expect(malformed.next()).rejects.toThrow();
    await expect(malformed.next()).rejects.toThrow();
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('forwards a partly consumed byte stream through the existing native adapter', async () => {
    read.mockResolvedValueOnce(batch([5, 251, 19], { closed: true }));
    const stream = readDurableByteStream({ url });
    expect((await stream.next()).value).toBe(5);
    const itemCodec = compileSchema(s.u8());
    const forwarded = agentStreamFromHandle<number>(
      agentStreamToHandle(stream, itemCodec),
      itemCodec,
    );
    expect((await forwarded.next()).value).toBe(251);
    expect((await forwarded.next()).value).toBe(19);
    expect((await forwarded.next()).done).toBe(true);
    expect(read).toHaveBeenCalledTimes(1);
    expect(readerHandles[0][Symbol.dispose]).toHaveBeenCalledTimes(1);
  });

  it('preserves local failures and enforces single-reader ownership', async () => {
    let finish!: (value: DurableStreamBatch) => void;
    read.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const stream = readDurableByteStream({ url });
    const next = stream.next();
    await expect(stream.next()).rejects.toThrow('already in progress');
    await expect(stream.return()).rejects.toThrow('already in progress');
    expect(readerHandles[0][Symbol.dispose]).not.toHaveBeenCalled();
    finish(batch([13, 37]));
    await next;
    await stream.return();
    expect(readerHandles[0][Symbol.dispose]).toHaveBeenCalledTimes(1);
    expect(read).toHaveBeenCalledTimes(1);
    read.mockRejectedValueOnce(failure('gone'));
    await expect(readDurableByteStream({ url }).next()).rejects.toMatchObject({ kind: 'gone' });
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe('external Durable Stream writers', () => {
  it('resolves uncertain data before disposal and retains the handle if resolution fails', async () => {
    const writer = createDurableByteWriter({ url, producerId: 'dispose', maxRetries: 0 });
    append.mockRejectedValueOnce(failure('transport')).mockRejectedValueOnce(failure('transport'));
    await expect(writer.append(new Uint8Array([19, 53]))).rejects.toMatchObject({
      kind: 'transport',
    });
    await expect(writer.dispose()).rejects.toMatchObject({ kind: 'transport' });
    expect(writerHandles[0][Symbol.dispose]).not.toHaveBeenCalled();
    append.mockResolvedValueOnce(receipt({ nextOffset: undefined }));
    await writer.dispose();
    await writer.dispose();
    expect(writerHandles[0][Symbol.dispose]).toHaveBeenCalledTimes(1);
    expect(append.mock.calls.map(([request]) => request)).toEqual(
      Array(3).fill({
        payload: { tag: 'bytes', val: new Uint8Array([19, 53]) },
        sequence: 0n,
        close: false,
      }),
    );
    expect(append.mock.contexts.every((handle) => handle === writerHandles[0])).toBe(true);
    expect(writers).toHaveBeenCalledTimes(1);
    expect(writer.nextSequence).toBe(1n);
    await expect(writer.append(new Uint8Array([83]))).rejects.toMatchObject({ kind: 'closed' });
    expect(append).toHaveBeenCalledTimes(3);
  });

  it('queues disposal behind an active append without closing the remote stream', async () => {
    let finish!: (value: DurableStreamAppendReceipt) => void;
    append.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const writer = createDurableByteWriter({ url });
    const writing = writer.append(new Uint8Array([71]));
    const disposing = writer.dispose();
    await vi.advanceTimersByTimeAsync(0);
    expect(writerHandles[0][Symbol.dispose]).not.toHaveBeenCalled();
    finish(receipt());
    await writing;
    await disposing;
    expect(writerHandles[0][Symbol.dispose]).toHaveBeenCalledTimes(1);
    expect(append).toHaveBeenCalledTimes(1);
    expect(append.mock.calls[0][0].close).toBe(false);
  });

  it('rejects encoding failures asynchronously without assigning a tuple', async () => {
    const writer = createDurableJsonWriter(s.u64(), { url });
    const result = writer.append([18446744073709551615n]);
    expect(result).toBeInstanceOf(Promise);
    await expect(result).rejects.toThrow('losslessly');
    expect(writer.nextSequence).toBe(0n);
    expect(append).not.toHaveBeenCalled();
  });

  it('hands the host complete JSON messages, not a preframed or flattened batch', async () => {
    append.mockResolvedValue(receipt());
    const writer = createDurableJsonWriter(z.array(z.number().int()), { url, producerId: 'json' });
    await writer.append([
      [2, 11],
      [7, 19],
    ]);
    expect(append.mock.calls[0][0].payload).toEqual({ tag: 'json', val: ['[2,11]', '[7,19]'] });
    const exact = createDurableJsonWriter(s.u64(), { url, encode: (value) => value.toString() });
    await exact.append([18446744073709551615n]);
    expect(append.mock.calls[1][0].payload).toEqual({ tag: 'json', val: ['18446744073709551615'] });
  });

  it('retries identical bytes/tuple/close with backoff and duplicate acknowledgement', async () => {
    append
      .mockRejectedValueOnce(failure('rate-limited', 730n))
      .mockResolvedValueOnce(receipt({ nextOffset: undefined, closed: true }));
    const auth = {} as Secret;
    const options = { url, producerId: 'stable', auth };
    const writer = createDurableByteWriter(options);
    const data = new Uint8Array([3, 241, 27]);
    const operation = writer.append(data, { close: true });
    data.fill(0);
    options.url = 'https://other.example';
    await vi.advanceTimersByTimeAsync(729);
    expect(append).toHaveBeenCalledTimes(1);
    expect(writer.nextSequence).toBe(0n);
    await vi.advanceTimersByTimeAsync(1);
    expect(await operation).toEqual({
      nextOffset: undefined,
      epoch: 0n,
      sequence: 0n,
      closed: true,
    });
    const [first, second] = append.mock.calls;
    expect(second[0]).toEqual(first[0]);
    expect(second[0]).toEqual({
      sequence: 0n,
      close: true,
      payload: { tag: 'bytes', val: new Uint8Array([3, 241, 27]) },
    });
    expect(writers).toHaveBeenCalledExactlyOnceWith(
      {
        url,
        producerId: 'stable',
        producerEpoch: 0n,
        contentType: 'application/octet-stream',
        timeoutMs: 30000n,
      },
      auth,
    );
    expect(append.mock.contexts).toEqual([writerHandles[0], writerHandles[0]]);
    expect(writerHandles[0][Symbol.dispose]).toHaveBeenCalledTimes(1);
    expect(writer.nextSequence).toBe(1n);
    await expect(writer.append(new Uint8Array([9]))).rejects.toMatchObject({ kind: 'closed' });
    expect(await writer.retryPending()).toBeUndefined();
    expect(append).toHaveBeenCalledTimes(2);
  });

  it('resolves a cancelled uncertain request before assigning different data', async () => {
    const cancelled = new Error('cancelled');
    append
      .mockRejectedValueOnce(cancelled)
      .mockResolvedValueOnce(receipt({ nextOffset: undefined }))
      .mockResolvedValueOnce(receipt({ sequence: 1n }));
    const writer = createDurableByteWriter({ url, producerId: 'one' });
    await expect(writer.append(new Uint8Array([17, 41]))).rejects.toBe(cancelled);
    expect((await writer.append(new Uint8Array([89]))).nextOffset).toBe('opaque:18');
    expect(append.mock.calls.map(([r]) => [r.sequence, [...r.payload.val]])).toEqual([
      [0n, [17, 41]],
      [0n, [17, 41]],
      [1n, [89]],
    ]);
    expect(writer.nextSequence).toBe(2n);
    expect(writers).toHaveBeenCalledTimes(1);
    expect(append.mock.contexts.every((handle) => handle === writerHandles[0])).toBe(true);
  });

  it('returns an offset-less retry acknowledgement and clears pending exactly once', async () => {
    const cancelled = new Error('cancelled');
    append
      .mockRejectedValueOnce(cancelled)
      .mockResolvedValueOnce({ epoch: 0n, sequence: 0n, closed: false });
    const writer = createDurableByteWriter({ url, producerId: 'retry' });
    await expect(writer.append(new Uint8Array([19]))).rejects.toBe(cancelled);
    const acknowledged = await writer.retryPending();
    expect(acknowledged).toEqual({ epoch: 0n, sequence: 0n, closed: false });
    expect(acknowledged?.nextOffset).toBeUndefined();
    expect(writer.nextSequence).toBe(1n);
    expect(await writer.retryPending()).toBeUndefined();
    expect(append).toHaveBeenCalledTimes(2);
  });

  it.each([undefined, 'opaque:close'])(
    'preserves fresh close-only offset %s without inferring duplication',
    async (nextOffset) => {
      append.mockResolvedValueOnce(receipt({ nextOffset, closed: true }));
      const writer = createDurableByteWriter({ url, producerId: 'close-only' });
      expect(await writer.close()).toEqual({ nextOffset, epoch: 0n, sequence: 0n, closed: true });
      expect(writer.nextSequence).toBe(1n);
      expect(append.mock.calls[0][0]).toMatchObject({
        close: true,
        payload: { tag: 'bytes', val: new Uint8Array() },
      });
      await expect(writer.append(new Uint8Array([31]))).rejects.toMatchObject({ kind: 'closed' });
      expect(append).toHaveBeenCalledTimes(1);
    },
  );

  it('serializes each writer while independent writers complete in reverse order', async () => {
    const active: {
      request: DurableStreamAppendRequest;
      resolve(value: DurableStreamAppendReceipt): void;
    }[] = [];
    append.mockImplementation(
      (request) =>
        new Promise((resolve) => {
          active.push({ request, resolve });
        }),
    );
    const first = createDurableByteWriter({ url, producerId: 'first' });
    const second = createDurableByteWriter({ url, producerId: 'second' });
    const a = first.append(new Uint8Array([13]));
    const b = first.append(new Uint8Array([31]));
    const c = second.append(new Uint8Array([79]));
    await vi.advanceTimersByTimeAsync(0);
    expect(writers.mock.calls.map(([options]) => options.producerId)).toEqual(['first', 'second']);
    expect(writerHandles[0]).not.toBe(writerHandles[1]);
    expect(append.mock.contexts).toEqual([writerHandles[0], writerHandles[1]]);
    active[1].resolve(receipt());
    await c;
    expect(first.nextSequence).toBe(0n);
    active[0].resolve(receipt());
    await a;
    await vi.advanceTimersByTimeAsync(0);
    expect(active[2].request.sequence).toBe(1n);
    expect(append.mock.contexts[2]).toBe(writerHandles[0]);
    active[2].resolve(receipt({ sequence: 1n }));
    await b;
  });

  it.each([
    ['fenced', 'fenced'],
    ['sequence-conflict', 'sequence-conflict'],
    ['payload-too-large', 'payload-too-large'],
    ['closed', 'closed'],
  ] as const)('does not retry or renumber %s', async (kind, expected) => {
    append.mockRejectedValue(failure(kind));
    const writer = createDurableByteWriter({ url, producerId: 'conflict' });
    await expect(writer.append(new Uint8Array([5]))).rejects.toMatchObject({ kind: expected });
    expect(writer.nextSequence).toBe(0n);
    expect(append).toHaveBeenCalledTimes(1);
  });

  it('surfaces diverged/invalid acknowledgements and retains the submitted tuple', async () => {
    const writer = createDurableByteWriter({ url, producerId: 'tuple', epoch: 7n });
    append
      .mockResolvedValueOnce(receipt({ epoch: 7n, sequence: 4n }))
      .mockResolvedValueOnce(receipt({ epoch: 8n }))
      .mockResolvedValueOnce(receipt({ epoch: 7n }));
    await expect(writer.append(new Uint8Array([7]))).rejects.toMatchObject({
      kind: 'producer-diverged',
    });
    await expect(writer.retryPending()).rejects.toMatchObject({ kind: 'protocol-error' });
    expect(writer.nextSequence).toBe(0n);
    await writer.retryPending();
    expect(writer.nextSequence).toBe(1n);
    expect(append.mock.calls.map(([r]) => r.sequence)).toEqual([0n, 0n, 0n]);
  });

  it('bounds automatic retries and rejects empty appends but sequences close-only', async () => {
    append.mockRejectedValue(failure('unavailable'));
    const writer = createDurableByteWriter({ url, producerId: 'bounded', maxRetries: 2 });
    const failed = expect(writer.append(new Uint8Array([7]))).rejects.toMatchObject({
      kind: 'unavailable',
    });
    await vi.runAllTimersAsync();
    await failed;
    expect(append).toHaveBeenCalledTimes(3);
    expect(writer.nextSequence).toBe(0n);
    append
      .mockResolvedValueOnce(receipt())
      .mockResolvedValueOnce(receipt({ sequence: 1n, closed: true }));
    await expect(writer.append(new Uint8Array())).rejects.toMatchObject({
      kind: 'invalid-request',
    });
    await writer.close();
    expect(append.mock.calls[4][0]).toMatchObject({
      sequence: 1n,
      close: true,
      payload: { tag: 'bytes', val: new Uint8Array() },
    });
    expect(writer.nextSequence).toBe(2n);
  });
});
