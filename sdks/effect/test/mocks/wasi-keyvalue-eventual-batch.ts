/**
 * Vitest stub for `wasi:keyvalue/eventual-batch@0.1.0`.
 *
 * Required only for `KeyValueLive` import resolution. Tests use
 * `test/host/KeyValueFake.ts` directly — these functions are not
 * exercised at runtime by any current test.
 */

import { Bucket, IncomingValue, OutgoingValue, __getBackingMap } from "./wasi-keyvalue-types.js"

export const getMany = (
  bucket: Bucket,
  keys: ReadonlyArray<string>,
): Array<IncomingValue | undefined> => {
  const map = __getBackingMap(bucket)
  return keys.map((k) => {
    const bytes = map.get(k)
    return bytes === undefined ? undefined : new IncomingValue(bytes)
  })
}

export const setMany = (
  bucket: Bucket,
  pairs: ReadonlyArray<readonly [string, OutgoingValue]>,
): void => {
  const map = __getBackingMap(bucket)
  for (const [k, ov] of pairs) {
    const bytes = (ov as { __bytes?: Uint8Array }).__bytes
    if (bytes === undefined) {
      throw new Error("OutgoingValue body not set before eventual-batch.set-many")
    }
    map.set(k, bytes)
  }
}

export const deleteMany = (bucket: Bucket, keys: ReadonlyArray<string>): void => {
  const map = __getBackingMap(bucket)
  for (const k of keys) map.delete(k)
}

export const keys = (bucket: Bucket): Array<string> => Array.from(__getBackingMap(bucket).keys())
