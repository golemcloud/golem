/**
 * Vitest stub for `wasi:keyvalue/eventual@0.1.0`.
 *
 * Required only for `KeyValueLive` import resolution. Tests use
 * `test/host/KeyValueFake.ts` directly — these functions are not
 * exercised at runtime by any current test.
 */

import { Bucket, IncomingValue, OutgoingValue, __getBackingMap } from "./wasi-keyvalue-types.js"

export const get = (bucket: Bucket, key: string): IncomingValue | undefined => {
  const bytes = __getBackingMap(bucket).get(key)
  if (bytes === undefined) return undefined
  return new IncomingValue(bytes)
}

export const set = (bucket: Bucket, key: string, ov: OutgoingValue): void => {
  const bytes = (ov as { __bytes?: Uint8Array }).__bytes
  if (bytes === undefined) {
    throw new Error("OutgoingValue body not set before eventual.set")
  }
  __getBackingMap(bucket).set(key, bytes)
}

export const delete_ = (bucket: Bucket, key: string): void => {
  __getBackingMap(bucket).delete(key)
}

export const exists = (bucket: Bucket, key: string): boolean => __getBackingMap(bucket).has(key)
