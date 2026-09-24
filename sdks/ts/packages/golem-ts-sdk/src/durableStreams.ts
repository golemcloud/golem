// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import {
  DurableStreamReader as HostReader,
  DurableStreamWriter as HostWriter,
  type DurableStreamAppendPayload,
  type DurableStreamAppendReceipt,
  type DurableStreamAppendRequest,
  type DurableStreamBatch,
  type DurableStreamError as HostError,
  type DurableStreamErrorKind,
  type DurableStreamReadRequest,
} from 'golem:agent/durable-streams@2.0.0';
import type { Secret as SecretHandle } from 'golem:core/types@2.0.0';
import { Secret } from './secret';
import { AgentStream } from './schema/agentStream';
import { compileSchema } from './schema/adapter';
import { SchemaRef } from './schema/ref';
import type { StandardSchemaV1 } from './schema/standardSchema';
import { decodeUtf8 } from './internal/utf8';
import { SECRET_INTERNAL } from './internal/schema-model/secretInternal';

export type { DurableStreamAppendReceipt, DurableStreamErrorKind };

/** A sanitized protocol failure. Native stream forwarding treats this as producer failure, not EOF. */
export class DurableStreamError extends Error {
  override readonly name = 'DurableStreamError';
  constructor(
    readonly kind: DurableStreamErrorKind,
    message: string,
    readonly retryAfterMs?: bigint,
    readonly producerEpoch?: bigint,
    readonly expectedSequence?: bigint,
  ) {
    super(message);
  }
}

export interface DurableStreamOptions {
  readonly url: string;
  /** Borrowed capability; the SDK never reveals the bearer token. */
  readonly auth?: Secret<string>;
  /** Whole HTTP attempt deadline, 1–300000 ms. Default: 30000. */
  readonly timeoutMs?: number;
  /** Automatic retries per batch operation, not counting the first attempt. Default: 5. */
  readonly maxRetries?: number;
  /** Initial exponential retry delay. Default: 100 ms; capped at 30000 ms. */
  readonly retryDelayMs?: number;
}

export interface DurableStreamReadOptions extends DurableStreamOptions {
  /** Opaque server offset. Default: -1. `now` is resolved once via catch-up. */
  readonly offset?: string;
  readonly cursor?: string;
  /** Live transport after catch-up. Default: long-poll. */
  readonly live?: 'long-poll' | 'sse';
  /** Durable delay after an empty live response. Default: 100 ms. */
  readonly idleDelayMs?: number;
}

export interface DurableStreamWriteOptions extends DurableStreamOptions {
  /** Generated once with the runtime's durable crypto.randomUUID() when omitted. */
  readonly producerId?: string;
  /** Nonnegative protocol integer, at most 2^53-1. Default: 0. A new epoch starts at sequence 0. */
  readonly epoch?: bigint;
  readonly contentType?: string;
}

/**
 * Optional exact application conversion. Each callback receives/returns one complete JSON value,
 * never the protocol's outer array. Callbacks must be synchronous and deterministic. The schema
 * still validates the resulting application value. Supply these for unquoted 64-bit integers;
 * the default canonical JSON codec rejects integers outside JavaScript's safe-number range.
 */
export interface DurableStreamJsonCodec<T> {
  readonly decode?: (json: string) => T;
  readonly encode?: (value: T) => string;
}

export interface DurableStreamWriter<T> {
  readonly producerId: string;
  readonly epoch: bigint;
  readonly nextSequence: bigint;
  /** Appends data, optionally closing atomically. Empty data requires close=true. */
  append(data: T, options?: { readonly close?: boolean }): Promise<DurableStreamAppendReceipt>;
  /** Close-only also consumes a sequence. */
  close(): Promise<DurableStreamAppendReceipt>;
  /** Resolve the retained uncertain request without assigning another sequence. */
  retryPending(): Promise<DurableStreamAppendReceipt | undefined>;
  /** Resolve pending data, then release the local handle without closing the remote stream. */
  dispose(): Promise<void>;
}

/** Read JSON messages lazily as an ordinary affine AgentStream. */
export function readDurableJsonStream<T>(
  schema: StandardSchemaV1<unknown, T>,
  options: DurableStreamReadOptions & Pick<DurableStreamJsonCodec<T>, 'decode'>,
): AgentStream<T> {
  const codec = compileSchema(schema);
  const ref = new SchemaRef(codec.graph);
  const decode = options.decode;
  return readStream(options, 'json', (payload) => {
    const messages = splitJsonBatch(decodeUtf8(payload));
    return {
      length: messages.length,
      at(index: number): T {
        if (!decode) return codec.fromValue(ref.packJson(JSON.parse(messages[index]))) as T;
        const value = decode(messages[index]);
        if (!ref.validateValue(codec.toValue(value)).success) {
          throw new TypeError('Decoded Durable Stream message does not conform to its schema');
        }
        return value;
      },
    };
  });
}

/** Read bytes, not append chunks: suitable for s.stream(s.u8()). */
export function readDurableByteStream(options: DurableStreamReadOptions): AgentStream<number> {
  return readStream(options, 'bytes', (payload) => ({
    length: payload.length,
    at: (index) => payload[index],
  }));
}

/** Write a batch of logical JSON messages; an array-valued message remains one message. */
export function createDurableJsonWriter<T>(
  schema: StandardSchemaV1<unknown, T>,
  options: DurableStreamWriteOptions & Pick<DurableStreamJsonCodec<T>, 'encode'>,
): DurableStreamWriter<readonly T[]> {
  const codec = compileSchema(schema);
  const ref = new SchemaRef(codec.graph);
  const encode = options.encode;
  return createWriter(options, 'application/json', { tag: 'json', val: [] }, (values) => ({
    tag: 'json',
    val: values.map((value) => {
      const packed = codec.toValue(value);
      if (!ref.validateValue(packed).success) {
        throw new TypeError('Durable Stream message does not conform to its schema');
      }
      const json = encode ? encode(value) : JSON.stringify(ref.unpackJson(packed));
      // Validate completeness without using the potentially rounded parsed value.
      JSON.parse(json);
      return json;
    }),
  }));
}

/** Write exact bytes. Remote readers cannot recover these append boundaries. */
export function createDurableByteWriter(
  options: DurableStreamWriteOptions,
): DurableStreamWriter<Uint8Array> {
  return createWriter(
    options,
    'application/octet-stream',
    { tag: 'bytes', val: new Uint8Array() },
    (value) => ({ tag: 'bytes', val: value.slice() }),
  );
}

function readStream<T>(
  input: DurableStreamReadOptions,
  mode: 'json' | 'bytes',
  decode: (payload: Uint8Array) => { length: number; at(index: number): T },
): AgentStream<T> {
  const options = checkedOptions(input);
  const live = input.live ?? 'long-poll';
  const idleDelay = boundedInteger(input.idleDelayMs ?? 100, 1, 300000, 'idleDelayMs');
  const reader = withAuth(
    options.auth,
    (auth) =>
      new HostReader({ url: options.url, mode, timeoutMs: BigInt(options.timeoutMs) }, auth),
  );
  const request: DurableStreamReadRequest = {
    checkpoint: { offset: input.offset ?? '-1', cursor: input.cursor },
    transport: 'catch-up',
    contentType: undefined,
  };
  let pending:
    | { batch: DurableStreamBatch; items?: ReturnType<typeof decode>; index: number }
    | undefined;
  let closed = false;
  let idle = false;
  let disposed = false;
  const dispose = () => {
    if (disposed) return;
    disposed = true;
    (reader as unknown as { [Symbol.dispose](): void })[Symbol.dispose]();
  };
  return AgentStream.from({
    [Symbol.asyncIterator]() {
      return {
        async next(): Promise<IteratorResult<T>> {
          while (!closed) {
            if (pending) {
              pending.items ??= decode(pending.batch.payload);
              if (pending.index < pending.items.length) {
                const value = pending.items.at(pending.index);
                pending.index += 1;
                return { done: false, value };
              }
              request.checkpoint = pending.batch.next;
              closed = pending.batch.closed;
              if (pending.batch.upToDate) request.transport = live;
              idle = pending.items.length === 0 && request.transport !== 'catch-up';
              pending = undefined;
              if (closed) break;
            }
            if (idle) {
              await delay(idleDelay);
              idle = false;
            }
            const batch = await retry(options, () => reader.read(request));
            // Install payload + next checkpoint before another await. Decode errors cannot skip it.
            pending = { batch, index: 0 };
            request.contentType ??= batch.contentType;
          }
          dispose();
          return { done: true, value: undefined };
        },
        async return(): Promise<IteratorResult<T>> {
          closed = true;
          pending = undefined;
          dispose();
          return { done: true, value: undefined };
        },
      };
    },
  });
}

function createWriter<T>(
  input: DurableStreamWriteOptions,
  defaultContentType: string,
  empty: DurableStreamAppendPayload,
  encode: (data: T) => DurableStreamAppendPayload,
): DurableStreamWriter<T> {
  const options = checkedOptions(input);
  const producerId = input.producerId ?? crypto.randomUUID();
  const epoch = input.epoch ?? 0n;
  const contentType = input.contentType ?? defaultContentType;
  if (!producerId || epoch < 0n || epoch > MAX_PRODUCER_INTEGER) {
    throw new DurableStreamError('invalid-request', 'Invalid producer ID or epoch');
  }
  const writer = withAuth(
    options.auth,
    (auth) =>
      new HostWriter(
        {
          url: options.url,
          contentType,
          producerId,
          producerEpoch: epoch,
          timeoutMs: BigInt(options.timeoutMs),
        },
        auth,
      ),
  );
  let nextSequence = 0n;
  let closed = false;
  let disposed = false;
  const dispose = () => {
    if (disposed) return;
    disposed = true;
    (writer as unknown as { [Symbol.dispose](): void })[Symbol.dispose]();
  };
  let pending: DurableStreamAppendRequest | undefined;
  let queue = Promise.resolve();
  const serialized = <R>(operation: () => Promise<R>): Promise<R> => {
    const result = queue.then(operation);
    queue = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  };
  const resolvePending = async (): Promise<DurableStreamAppendReceipt | undefined> => {
    if (!pending) return undefined;
    const request = pending;
    const receipt = await retry(options, () => writer.append(request));
    if (receipt.epoch !== epoch || receipt.sequence < request.sequence) {
      throw new DurableStreamError('protocol-error', 'Invalid producer acknowledgement');
    }
    if (receipt.sequence > request.sequence) {
      throw new DurableStreamError('producer-diverged', 'Another writer advanced this producer');
    }
    nextSequence = request.sequence + 1n;
    closed = receipt.closed;
    pending = undefined;
    if (closed) dispose();
    return receipt;
  };
  const append = (payload: DurableStreamAppendPayload, close: boolean) =>
    serialized(async () => {
      await resolvePending();
      if (closed || disposed)
        throw new DurableStreamError('closed', 'Durable Stream writer is closed');
      if (nextSequence > MAX_PRODUCER_INTEGER) {
        throw new DurableStreamError('sequence-conflict', 'Producer sequence exhausted');
      }
      pending = {
        payload,
        sequence: nextSequence,
        close,
      };
      return (await resolvePending())!;
    });
  return {
    producerId,
    epoch,
    get nextSequence() {
      return nextSequence;
    },
    async append(data, appendOptions) {
      const payload = encode(data);
      const close = appendOptions?.close ?? false;
      if (payload.val.length === 0 && !close) {
        throw new DurableStreamError('invalid-request', 'Empty append requires close');
      }
      return append(payload, close);
    },
    close: () => append(empty, true),
    retryPending: () => serialized(resolvePending),
    dispose: () =>
      serialized(async () => {
        await resolvePending();
        dispose();
      }),
  };
}

const MAX_PRODUCER_INTEGER = 9007199254740991n;

function withAuth<R>(auth: Secret<string> | undefined, use: (handle?: SecretHandle) => R): R {
  return auth === undefined ? use() : auth[SECRET_INTERNAL](use);
}

function boundedInteger(value: number, min: number, max: number, name: string): number {
  if (!Number.isSafeInteger(value) || value < min || value > max) {
    throw new DurableStreamError(
      'invalid-request',
      `${name} must be an integer from ${min} through ${max}`,
    );
  }
  return value;
}

function checkedOptions(options: DurableStreamOptions) {
  return {
    url: options.url,
    auth: options.auth,
    timeoutMs: boundedInteger(options.timeoutMs ?? 30000, 1, 300000, 'timeoutMs'),
    maxRetries: boundedInteger(options.maxRetries ?? 5, 0, 1000, 'maxRetries'),
    retryDelayMs: boundedInteger(options.retryDelayMs ?? 100, 1, 30000, 'retryDelayMs'),
  };
}

function hostError(error: unknown): DurableStreamError | undefined {
  if (error instanceof DurableStreamError) return error;
  if (error === null || typeof error !== 'object') return undefined;
  if (!('kind' in error) || !('message' in error)) return undefined;
  const typed = error as HostError;
  return new DurableStreamError(
    typed.kind,
    typed.message,
    typed.retryAfterMs,
    typed.producerEpoch,
    typed.expectedSequence,
  );
}

async function retry<T>(
  options: ReturnType<typeof checkedOptions>,
  action: () => Promise<T>,
): Promise<T> {
  for (let failures = 0; ; failures += 1) {
    try {
      return await action();
    } catch (cause) {
      const error = hostError(cause);
      if (!error) throw cause;
      if (
        failures >= options.maxRetries ||
        !['timeout', 'transport', 'rate-limited', 'unavailable'].includes(error.kind)
      ) {
        throw error;
      }
      const backoff = Math.min(30000, options.retryDelayMs * 2 ** failures);
      // Never retry sooner than Retry-After. Split long waits to avoid JS timer overflow.
      await delay(
        error.retryAfterMs !== undefined && error.retryAfterMs > BigInt(backoff)
          ? error.retryAfterMs
          : BigInt(backoff),
      );
    }
  }
}

async function delay(milliseconds: number | bigint): Promise<void> {
  let remaining = BigInt(milliseconds);
  while (remaining > 0n) {
    const chunk = remaining > 2147483647n ? 2147483647 : Number(remaining);
    await new Promise<void>((resolve) => globalThis.setTimeout(resolve, chunk));
    remaining -= BigInt(chunk);
  }
}

/** Keep original number lexemes for application decoders instead of parsing and re-stringifying. */
function splitJsonBatch(json: string): string[] {
  if (json.length === 0) return [];
  const text = json.trim();
  if (text[0] !== '[' || text[text.length - 1] !== ']') {
    throw new DurableStreamError('protocol-error', 'JSON read payload must be an array');
  }
  if (text.slice(1, -1).trim() === '') return [];
  const values: string[] = [];
  let depth = 0;
  let quoted = false;
  let start = 1;
  for (let index = 1; index < text.length - 1; index += 1) {
    const char = text[index];
    if (quoted) {
      if (char === '\\') index += 1;
      else if (char === '"') quoted = false;
    } else if (char === '"') quoted = true;
    else if (char === '[' || char === '{') depth += 1;
    else if (char === ']' || char === '}') depth -= 1;
    else if (char === ',' && depth === 0) {
      values.push(text.slice(start, index));
      start = index + 1;
    }
  }
  values.push(text.slice(start, -1));
  for (const value of values) JSON.parse(value);
  return values;
}
