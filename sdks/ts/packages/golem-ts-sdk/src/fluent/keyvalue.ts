// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Plain-async fluent wrapper around the `wasi:keyvalue@0.1.0` host interface.
// Every operation returns a `Promise` (or a plain value where the host call is
// synchronous) and throws a typed `KeyValueError` on failure.
//
// Only the `eventual` (single-key CRUD) and `eventual-batch` (multi-key CRUD)
// interfaces are exposed. The `atomic` and `cache` interfaces are NOT wrapped:
// they are `unimplemented!` in the Golem host and trap the worker.

import * as KvBatch from 'wasi:keyvalue/eventual-batch@0.1.0';
import { strictTextDecoder } from './strictTextDecoder';
import * as KvEventual from 'wasi:keyvalue/eventual@0.1.0';
import * as KvTypes from 'wasi:keyvalue/types@0.1.0';
import { compileSchema } from './schema/adapter';
import type { StandardSchemaV1 } from './schema/standardSchema';

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

const traceOf = (e: unknown): string => {
  if (e !== null && typeof e === 'object') {
    const obj = e as { trace?: () => string; message?: string };
    if (typeof obj.trace === 'function') {
      try {
        return obj.trace();
      } catch {
        /* fall through */
      }
    }
    if (typeof obj.message === 'string') return obj.message;
  }
  return String(e);
};

/**
 * Raised when any host call into `wasi:keyvalue@0.1.0` traps. The `trace`
 * field carries the verbatim driver-supplied string from the host's
 * `error.trace()` (or, for non-Error host throws, the stringified cause). Per
 * the Golem source this is opaque / driver-specific and should not be parsed.
 */
export class KeyValueError extends Error {
  override readonly name = 'KeyValueError';
  readonly trace: string;
  readonly operation: string;
  constructor(
    readonly cause: unknown,
    operation: string,
  ) {
    const t = traceOf(cause);
    super(`KeyValueError(${operation}): ${t}`);
    this.trace = t;
    this.operation = operation;
  }
}

const wrap = <A>(operation: string, fn: () => A): A => {
  try {
    return fn();
  } catch (cause) {
    if (cause instanceof KeyValueError) throw cause;
    throw new KeyValueError(cause, operation);
  }
};

// ---------------------------------------------------------------------------
// Schema codec — UTF-8 JSON over a Standard Schema
// ---------------------------------------------------------------------------
//
// The fluent SDK has no Effect Schema; we validate values through the Standard
// Schema's synchronous `~standard.validate` and JSON-encode the validated
// value to UTF-8 bytes. Validation/JSON failures throw {@link KeyValueError}.

const textEncoder = new TextEncoder();
const textDecoder = strictTextDecoder();

const validateSync = <T>(schema: StandardSchemaV1, value: unknown): T => {
  const result = schema['~standard'].validate(value);
  if (result instanceof Promise) {
    throw new Error('Schema validation must be synchronous for keyvalue payloads');
  }
  if (result.issues !== undefined) {
    const msg = result.issues.map((i) => i.message).join('; ');
    throw new Error(`Schema validation failed: ${msg}`);
  }
  return result.value as T;
};

/** Encode a schema-validated value to UTF-8 JSON bytes. */
const encodeValue = <T>(schema: StandardSchemaV1, value: T): Uint8Array =>
  wrap('schema.encode', () => {
    const validated = validateSync<T>(schema, value);
    return textEncoder.encode(JSON.stringify(validated));
  });

/** Decode UTF-8 JSON bytes and re-validate against the schema. */
const decodeValue = <T>(schema: StandardSchemaV1, bytes: Uint8Array): T =>
  wrap('schema.decode', () => {
    const json = textDecoder.decode(bytes);
    const parsed: unknown = JSON.parse(json);
    return validateSync<T>(schema, parsed);
  });

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * Schema-typed view of a {@link Bucket}. Values are validated against the
 * Standard Schema and JSON-encoded (UTF-8) on writes; reads JSON-parse and
 * re-validate. Decoding failures throw {@link KeyValueError}.
 */
export interface SchemaBucket<T> {
  get(key: string): T | undefined;
  set(key: string, value: T): void;
  delete(key: string): void;
  exists(key: string): boolean;
  getMany(keys: readonly string[]): Array<T | undefined>;
  setMany(entries: ReadonlyArray<readonly [string, T]>): void;
  deleteMany(keys: readonly string[]): void;
  keys(): string[];
}

/**
 * Handle on an open keyvalue bucket. Acquired via {@link openBucket}. The
 * underlying WIT `bucket` resource has no explicit close method; dropping the
 * JS handle is enough (GC-managed lifetime).
 *
 * All host calls are synchronous, so these methods return plain values rather
 * than Promises.
 */
export interface Bucket {
  readonly name: string;

  /** Get the bytes stored under `key`, or `undefined` if absent. */
  get(key: string): Uint8Array | undefined;
  /** Overwrite `key` with `value`, or insert if absent. */
  set(key: string, value: Uint8Array): void;
  /** Remove `key`. No-op if absent. */
  delete(key: string): void;
  /** Test whether `key` is present. */
  exists(key: string): boolean;

  /**
   * Batch-get. Returned array length === `keys.length`; missing keys surface
   * as `undefined`, positionally aligned with the request. All-or-nothing: a
   * storage-layer error fails the whole call.
   */
  getMany(keys: readonly string[]): Array<Uint8Array | undefined>;
  /** Batch-set. Per-entry order is not guaranteed; failure is partial. */
  setMany(entries: ReadonlyArray<readonly [string, Uint8Array]>): void;
  /** Batch-delete. Per-entry order is not guaranteed; failure is partial. */
  deleteMany(keys: readonly string[]): void;

  /** All keys currently present in the bucket. Order undefined. */
  keys(): string[];

  /**
   * Build a schema-typed view of this bucket. Values are JSON-encoded via the
   * supplied Standard Schema on writes and re-validated on reads.
   */
  forSchema<S extends StandardSchemaV1>(
    schema: S,
  ): SchemaBucket<StandardSchemaV1.InferOutput<S>>;
}

// ---------------------------------------------------------------------------
// Host plumbing helpers (IncomingValue / OutgoingValue chunking folded in)
// ---------------------------------------------------------------------------

const writeOutgoing = (bytes: Uint8Array): KvTypes.OutgoingValue =>
  wrap('outgoingValueWriteBodySync', () => {
    const ov = KvTypes.OutgoingValue.newOutgoingValue();
    ov.outgoingValueWriteBodySync(bytes);
    return ov;
  });

const consumeIncoming = (iv: KvTypes.IncomingValue): Uint8Array =>
  wrap('incomingValueConsumeSync', () => iv.incomingValueConsumeSync());

// ---------------------------------------------------------------------------
// SchemaBucket factory
// ---------------------------------------------------------------------------

const makeSchemaBucket = <T>(bucket: Bucket, schema: StandardSchemaV1): SchemaBucket<T> => ({
  get(key) {
    const raw = bucket.get(key);
    return raw === undefined ? undefined : decodeValue<T>(schema, raw);
  },
  set(key, value) {
    bucket.set(key, encodeValue(schema, value));
  },
  delete(key) {
    bucket.delete(key);
  },
  exists(key) {
    return bucket.exists(key);
  },
  getMany(keys) {
    return bucket
      .getMany(keys)
      .map((raw) => (raw === undefined ? undefined : decodeValue<T>(schema, raw)));
  },
  setMany(entries) {
    bucket.setMany(entries.map(([k, v]) => [k, encodeValue(schema, v)] as const));
  },
  deleteMany(keys) {
    bucket.deleteMany(keys);
  },
  keys() {
    return bucket.keys();
  },
});

// ---------------------------------------------------------------------------
// Bucket factory
// ---------------------------------------------------------------------------

const makeBucket = (name: string, handle: KvTypes.Bucket): Bucket => {
  const self: Bucket = {
    name,
    get(key) {
      const iv = wrap('eventual.get', () => KvEventual.get(handle, key));
      if (iv === undefined || iv === null) return undefined;
      return consumeIncoming(iv);
    },
    set(key, value) {
      const ov = writeOutgoing(value);
      wrap('eventual.set', () => KvEventual.set(handle, key, ov));
    },
    delete(key) {
      wrap('eventual.delete', () => KvEventual.delete_(handle, key));
    },
    exists(key) {
      return wrap('eventual.exists', () => KvEventual.exists(handle, key));
    },
    getMany(keys) {
      const raw = wrap(
        'eventual-batch.get-many',
        () =>
          KvBatch.getMany(handle, [...keys]) as ReadonlyArray<KvTypes.IncomingValue | undefined>,
      );
      return raw.map((iv) => (iv === undefined || iv === null ? undefined : consumeIncoming(iv)));
    },
    setMany(entries) {
      const pairs: Array<[string, KvTypes.OutgoingValue]> = entries.map(([k, v]) => [
        k,
        writeOutgoing(v),
      ]);
      wrap('eventual-batch.set-many', () => KvBatch.setMany(handle, pairs));
    },
    deleteMany(keys) {
      wrap('eventual-batch.delete-many', () => KvBatch.deleteMany(handle, [...keys]));
    },
    keys() {
      return wrap('eventual-batch.keys', () => KvBatch.keys(handle));
    },
    forSchema<S extends StandardSchemaV1>(schema: S) {
      // compileSchema validates the schema is a real, registered Standard
      // Schema (throwing a descriptive error otherwise) before we build the
      // JSON byte view over it.
      compileSchema(schema);
      return makeSchemaBucket<StandardSchemaV1.InferOutput<S>>(self, schema);
    },
  };
  return self;
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Open a bucket by name. The WIT bucket resource has no explicit close; the
 * returned handle's lifetime is GC-managed. Failures from the host's
 * `bucket.open-bucket` surface as {@link KeyValueError}.
 *
 * `openBucket` is async for API ergonomics, although the host call is
 * synchronous.
 */
export async function openBucket(name: string): Promise<Bucket> {
  const handle = wrap('openBucket', () => KvTypes.Bucket.openBucket(name));
  return makeBucket(name, handle);
}
