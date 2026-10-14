/**
 * Vitest stub for `wasi:keyvalue/types@0.1.0`.
 *
 * Only required for {@link import("../../src/host/KeyValueClient.js").KeyValueLive}
 * to resolve the WIT specifier when {@link import("../../src/host/HostLive.js").HostLive}
 * is built (e.g. by dispatcher tests like `agent.test.ts`). No test
 * actively exercises this path through `KeyValueLive` — see
 * `test/host/KeyValueFake.ts` for the Layer-based test fake used by
 * `test/keyvalue.test.ts`.
 */

const __buckets: Map<string, Map<string, Uint8Array>> = new Map()

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
