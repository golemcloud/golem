/**
 * Vitest mock for `wasi:blobstore/types`.
 *
 * `OutgoingValue` collects bytes via `outgoingValueWriteBody()` →
 * an `OutputStream` whose buffer is shared with the
 * `OutgoingValue` instance (mirrors the real host behaviour where
 * `write-data` reads from the shared `Arc<RwLock<Vec<u8>>>`).
 *
 * `IncomingValue` wraps a `Uint8Array` and supports the sync
 * consume path used by the SDK.
 */

export type ContainerName = string
export type ObjectName = string
export type Timestamp = bigint
export type ObjectSize = bigint
export type Error = string

export interface ContainerMetadata {
  name: ContainerName
  createdAt: Timestamp
}

export interface ObjectMetadata {
  name: ObjectName
  container: ContainerName
  createdAt: Timestamp
  size: ObjectSize
}

export interface ObjectId {
  container: ContainerName
  object: ObjectName
}

export class OutgoingValue {
  /** @internal */
  readonly __bytes: number[] = []
  static newOutgoingValue(): OutgoingValue {
    return new OutgoingValue()
  }
  outgoingValueWriteBody(data: AsyncIterable<number>): void {
    void (async () => {
      for await (const byte of data) this.__bytes.push(byte)
    })()
  }
  /** Test helper: read the bytes written so far. */
  __getBytes(): Uint8Array {
    return new Uint8Array(this.__bytes)
  }
}

export class IncomingValue {
  constructor(private readonly _bytes: Uint8Array) {}
  incomingValueConsumeSync(): Uint8Array {
    return this._bytes
  }
  incomingValueConsumeAsync(): never {
    throw new Error("incomingValueConsumeAsync not implemented in mock")
  }
  size(): bigint {
    return BigInt(this._bytes.length)
  }
}

export type IncomingValueAsyncBody = unknown
export type IncomingValueSyncBody = Uint8Array
