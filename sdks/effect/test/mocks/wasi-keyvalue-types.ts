/**
 * Vitest mock for `wasi:keyvalue/types@0.1.0`.
 *
 * Backs `Bucket.openBucket(name)` with a per-name in-memory `Map`
 * shared across the entire test process. `OutgoingValue` collects
 * bytes written via `outgoingValueWriteBodySync`; `IncomingValue`
 * wraps a `Uint8Array` and exposes the standard sync/async consume
 * methods (only the sync path is exercised by the SDK).
 *
 * Tests reset state between runs via {@link __resetKeyValueMock}.
 */

export type Key = string

const __buckets: Map<string, Map<string, Uint8Array>> = new Map()
const __openErrors: Map<string, string> = new Map()

export const __resetKeyValueMock = (): void => {
  __buckets.clear()
  __openErrors.clear()
}

export const __seedBucket = (name: string, entries: Record<string, Uint8Array>): void => {
  const map = new Map<string, Uint8Array>()
  for (const [k, v] of Object.entries(entries)) map.set(k, v)
  __buckets.set(name, map)
}

export const __setOpenError = (name: string, trace: string): void => {
  __openErrors.set(name, trace)
}

/** @internal — shared with the eventual / batch mocks. */
export const __getBackingMap = (b: Bucket): Map<string, Uint8Array> => b.__map

export class Bucket {
  /** @internal */
  __map: Map<string, Uint8Array>
  constructor(public readonly name: string) {
    let map = __buckets.get(name)
    if (!map) {
      map = new Map()
      __buckets.set(name, map)
    }
    this.__map = map
  }
  static openBucket(name: string): Bucket {
    const trace = __openErrors.get(name)
    if (trace !== undefined) {
      throw new (class HostErr {
        readonly _hostErr = true
        constructor(public readonly _trace: string) {}
        trace() {
          return this._trace
        }
        get message() {
          return this._trace
        }
      })(trace)
    }
    return new Bucket(name)
  }
}

export class OutgoingValue {
  /** @internal */
  __bytes: Uint8Array | undefined
  static newOutgoingValue(): OutgoingValue {
    return new OutgoingValue()
  }
  outgoingValueWriteBodySync(value: Uint8Array): void {
    this.__bytes = value
  }
  outgoingValueWriteBodyAsync(): never {
    throw new Error("outgoingValueWriteBodyAsync not implemented in mock")
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
  incomingValueSize(): bigint {
    return BigInt(this._bytes.length)
  }
}

export type IncomingValueAsyncBody = unknown
export type IncomingValueSyncBody = Uint8Array
export type OutgoingValueBodyAsync = unknown
export type OutgoingValueBodySync = Uint8Array
