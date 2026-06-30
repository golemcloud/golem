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

// Plain-async (no Effect) fluent wrapper around `wasi:blobstore/blobstore` +
// `wasi:blobstore/container` + `wasi:blobstore/types`. Ported from
// effect-golem's `Blobstore.ts` + `host/BlobstoreClient.ts`, de-Effect-ified:
// operations return `Promise`s and throw a typed `BlobstoreError` instead of
// failing an Effect.

import * as Blob from 'wasi:blobstore/blobstore';
import * as ContainerNS from 'wasi:blobstore/container';
import * as Types from 'wasi:blobstore/types';
import { compileSchema } from './schema/adapter';
import type { StandardSchemaV1 } from './schema/standardSchema';

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

const messageOf = (e: unknown): string => {
  if (e !== null && typeof e === 'object') {
    const obj = e as { message?: string };
    if (typeof obj.message === 'string') return obj.message;
  }
  if (typeof e === 'string') return e;
  return String(e);
};

/**
 * Raised when any host call into `wasi:blobstore/*` traps. The blobstore WIT
 * error type is a plain `string`, so {@link trace} carries it verbatim.
 */
export class BlobstoreError extends Error {
  override readonly name = 'BlobstoreError';
  readonly trace: string;
  readonly operation: string;
  constructor(
    readonly cause: unknown,
    operation: string,
  ) {
    const t = messageOf(cause);
    super(`BlobstoreError(${operation}): ${t}`);
    this.trace = t;
    this.operation = operation;
  }
}

const wrap = <A>(operation: string, fn: () => A): A => {
  try {
    return fn();
  } catch (cause) {
    if (cause instanceof BlobstoreError) throw cause;
    throw new BlobstoreError(cause, operation);
  }
};

// ---------------------------------------------------------------------------
// Schema codec — UTF-8 JSON over a Standard Schema
// ---------------------------------------------------------------------------

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder('utf-8', { fatal: true });

const validateSync = <T>(schema: StandardSchemaV1, value: unknown): T => {
  const result = schema['~standard'].validate(value);
  if (result instanceof Promise) {
    throw new Error('Schema validation must be synchronous for blobstore payloads');
  }
  if (result.issues !== undefined) {
    const msg = result.issues.map((i) => i.message).join('; ');
    throw new Error(`Schema validation failed: ${msg}`);
  }
  return result.value as T;
};

const encodeValue = <T>(schema: StandardSchemaV1, value: T): Uint8Array =>
  wrap('schema.encode', () => {
    const validated = validateSync<T>(schema, value);
    return textEncoder.encode(JSON.stringify(validated));
  });

const decodeValue = <T>(schema: StandardSchemaV1, bytes: Uint8Array): T =>
  wrap('schema.decode', () => {
    const json = textDecoder.decode(bytes);
    const parsed: unknown = JSON.parse(json);
    return validateSync<T>(schema, parsed);
  });

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/** Object identifier — `(containerName, objectName)` pair. */
export interface ObjectId {
  readonly container: string;
  readonly object: string;
}

/**
 * Container metadata. The WIT `created-at` `u64` is populated by the Golem
 * host as Unix milliseconds (it is actually `last_modified_at`; object stores
 * have no separate creation time). Exposed as both a JS `Date` and the raw
 * bigint.
 */
export interface ContainerMetadata {
  readonly name: string;
  readonly createdAt: Date;
  readonly createdAtMillis: bigint;
}

/** Object metadata. Same `created-at` caveat as {@link ContainerMetadata}. */
export interface ObjectMetadata {
  readonly name: string;
  readonly container: string;
  readonly createdAt: Date;
  readonly createdAtMillis: bigint;
  readonly size: bigint;
}

/**
 * Schema-typed view of a {@link Container}. Object bodies are JSON-encoded via
 * the supplied Standard Schema.
 */
export interface SchemaContainer<T> {
  getData(name: string): Promise<T>;
  writeData(name: string, value: T): Promise<void>;
  has(name: string): Promise<boolean>;
  info(name: string): Promise<ObjectMetadata>;
  delete(name: string): Promise<void>;
}

/**
 * Handle on an open blob container. Acquired via {@link createContainer},
 * {@link getContainer} or {@link getOrCreateContainer}. GC-managed lifetime.
 */
export interface Container {
  readonly name: string;

  /** Container metadata. */
  info(): Promise<ContainerMetadata>;
  /** Remove all objects, leaving the container empty. */
  clear(): Promise<void>;

  /**
   * Read an object's bytes. With no range, the whole object is read (with a
   * recovery retry for the host's inclusive/exclusive end divergence). With an
   * explicit `[start, end]` range the bytes are passed to the host verbatim.
   *
   * **Backend caveat.** The Golem host diverges across backends: in-memory +
   * filesystem treat `end` as exclusive (Rust range), S3 treats it as
   * inclusive. The WIT spec says inclusive. Ranged reads are not portable.
   */
  getData(name: string, start?: bigint, end?: bigint): Promise<Uint8Array>;
  /** Create or replace `name` with `data` (chunked at 4096 bytes per write). */
  writeData(name: string, data: Uint8Array): Promise<void>;

  /** True if the named object exists in this container. */
  has(name: string): Promise<boolean>;
  /** Metadata for the named object. Fails if the object does not exist. */
  objectInfo(name: string): Promise<ObjectMetadata>;
  /** Delete the named object. Does NOT fail if it does not exist. */
  delete(name: string): Promise<void>;
  /** Delete multiple objects. */
  deleteMany(names: readonly string[]): Promise<void>;

  /**
   * All object names in this container. The Golem host eagerly snapshots the
   * full name list when `list-objects` is called and pins it to the oplog;
   * this drains the whole snapshot (page size 256). Order is not guaranteed.
   */
  listObjects(): Promise<string[]>;

  /** Build a schema-typed view of this container. */
  forSchema<S extends StandardSchemaV1>(
    schema: S,
  ): SchemaContainer<StandardSchemaV1.InferOutput<S>>;
}

// ---------------------------------------------------------------------------
// Host plumbing helpers
// ---------------------------------------------------------------------------

const CHUNK_SIZE = 4096;
const LIST_PAGE_SIZE = 256n;

const consumeIncoming = (iv: Types.IncomingValue): Uint8Array =>
  wrap('incomingValueConsumeSync', () => iv.incomingValueConsumeSync());

const buildOutgoingValue = (bytes: Uint8Array): Types.OutgoingValue =>
  wrap('outgoingValueWriteBody', () => {
    const ov = Types.OutgoingValue.newOutgoingValue();
    const stream = ov.outgoingValueWriteBody();
    const total = bytes.length;
    let offset = 0;
    while (offset < total) {
      const remaining = total - offset;
      const chunkLen = remaining > CHUNK_SIZE ? CHUNK_SIZE : remaining;
      stream.blockingWriteAndFlush(bytes.subarray(offset, offset + chunkLen));
      offset += chunkLen;
    }
    return ov;
  });

const toContainerMetadata = (
  m: ContainerNS.ContainerMetadata | Types.ContainerMetadata,
): ContainerMetadata => ({
  name: m.name,
  createdAt: new Date(Number(m.createdAt)),
  createdAtMillis: m.createdAt,
});

const toObjectMetadata = (m: Types.ObjectMetadata): ObjectMetadata => ({
  name: m.name,
  container: m.container,
  createdAt: new Date(Number(m.createdAt)),
  createdAtMillis: m.createdAt,
  size: m.size,
});

const readRange = (handle: ContainerNS.Container, name: string, start: bigint, end: bigint): Uint8Array =>
  wrap('container.getData', () => {
    const iv = handle.getData(name, start, end);
    return consumeIncoming(iv);
  });

// ---------------------------------------------------------------------------
// SchemaContainer factory
// ---------------------------------------------------------------------------

const makeSchemaContainer = <T>(container: Container, schema: StandardSchemaV1): SchemaContainer<T> => ({
  async getData(name) {
    const bytes = await container.getData(name);
    return decodeValue<T>(schema, bytes);
  },
  async writeData(name, value) {
    await container.writeData(name, encodeValue(schema, value));
  },
  has(name) {
    return container.has(name);
  },
  info(name) {
    return container.objectInfo(name);
  },
  delete(name) {
    return container.delete(name);
  },
});

// ---------------------------------------------------------------------------
// Container factory
// ---------------------------------------------------------------------------

const makeContainer = (name: string, handle: ContainerNS.Container): Container => {
  const objectInfo = async (objectName: string): Promise<ObjectMetadata> => {
    const m = wrap('container.objectInfo', () => handle.objectInfo(objectName));
    return toObjectMetadata(m);
  };

  const self: Container = {
    name,
    async info() {
      const m = wrap('container.info', () => handle.info());
      return toContainerMetadata(m);
    },
    async clear() {
      wrap('container.clear', () => handle.clear());
    },
    async getData(objectName, start, end) {
      // Explicit range — pass through verbatim (backend semantics diverge).
      if (start !== undefined && end !== undefined) {
        return readRange(handle, objectName, start, end);
      }
      // Whole-object read. The WIT spec says `end` is inclusive, but the Golem
      // in-memory and filesystem backends treat it as exclusive. Tolerate both:
      const meta = await objectInfo(objectName);
      const size = meta.size;
      if (size === 0n) return new Uint8Array(0);
      const firstAttempt = readRange(handle, objectName, 0n, size - 1n);
      if (BigInt(firstAttempt.length) === size) return firstAttempt;
      // Backend treats `end` as exclusive — replay with end = size.
      return readRange(handle, objectName, 0n, size);
    },
    async writeData(objectName, data) {
      const ov = buildOutgoingValue(data);
      wrap('container.writeData', () => handle.writeData(objectName, ov));
    },
    async has(objectName) {
      return wrap('container.hasObject', () => handle.hasObject(objectName));
    },
    objectInfo,
    async delete(objectName) {
      wrap('container.deleteObject', () => handle.deleteObject(objectName));
    },
    async deleteMany(names) {
      wrap('container.deleteObjects', () => handle.deleteObjects([...names]));
    },
    async listObjects() {
      const iter = wrap('container.listObjects', () => handle.listObjects());
      const out: string[] = [];
      let done = false;
      while (!done) {
        const [names, end] = wrap('streamObjectNames.read', () =>
          iter.readStreamObjectNames(LIST_PAGE_SIZE),
        );
        out.push(...names);
        done = end;
      }
      return out;
    },
    forSchema<S extends StandardSchemaV1>(schema: S) {
      compileSchema(schema);
      return makeSchemaContainer<StandardSchemaV1.InferOutput<S>>(self, schema);
    },
  };
  return self;
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Create a new empty container. Fails if a container of the same name exists. */
export async function createContainer(name: string): Promise<Container> {
  const handle = wrap('createContainer', () => Blob.createContainer(name));
  return makeContainer(name, handle);
}

/** Open an existing container by name. Fails if it does not exist. */
export async function getContainer(name: string): Promise<Container> {
  const handle = wrap('getContainer', () => Blob.getContainer(name));
  return makeContainer(name, handle);
}

/** True if a container with the given name exists. */
export async function containerExists(name: string): Promise<boolean> {
  return wrap('containerExists', () => Blob.containerExists(name));
}

/**
 * Open the named container, creating it first if it does not exist.
 * `wasi:blobstore` has no atomic get-or-create primitive, so this optimistically
 * calls `createContainer` and, on failure, replays `getContainer` if
 * `containerExists` now reports the container present (collapses the TOCTOU
 * window). Otherwise the original create failure propagates.
 */
export async function getOrCreateContainer(name: string): Promise<Container> {
  try {
    return await createContainer(name);
  } catch (createErr) {
    let exists = false;
    try {
      exists = await containerExists(name);
    } catch {
      exists = false;
    }
    if (exists) return getContainer(name);
    throw createErr;
  }
}

/** Delete a container and all of its objects. */
export async function deleteContainer(name: string): Promise<void> {
  wrap('deleteContainer', () => Blob.deleteContainer(name));
}

/** Copy an object to the same or a different container. Overwrites the destination. */
export async function copyObject(src: ObjectId, dest: ObjectId): Promise<void> {
  wrap('copyObject', () => Blob.copyObject(src, dest));
}

/** Move (rename) an object. Overwrites the destination. */
export async function moveObject(src: ObjectId, dest: ObjectId): Promise<void> {
  wrap('moveObject', () => Blob.moveObject(src, dest));
}
