/**
 * Vitest mock for `wasi:keyvalue/eventual-batch@0.1.0`.
 */

import { Bucket, IncomingValue, OutgoingValue, __getBackingMap } from "./wasi-keyvalue-types.js"

let __nextError: { op: "get-many" | "set-many" | "delete-many" | "keys"; trace: string } | undefined

export const __setNextBatchError = (
  op: "get-many" | "set-many" | "delete-many" | "keys",
  trace: string,
): void => {
  __nextError = { op, trace }
}
export const __resetNextBatchError = (): void => {
  __nextError = undefined
}

const maybeThrow = (op: "get-many" | "set-many" | "delete-many" | "keys"): void => {
  if (__nextError && __nextError.op === op) {
    const trace = __nextError.trace
    __nextError = undefined
    const err = {
      _hostErr: true,
      _trace: trace,
      trace() {
        return trace
      },
      message: trace,
    }
    throw err
  }
}

export const getMany = (
  bucket: Bucket,
  keys: ReadonlyArray<string>,
): Array<IncomingValue | undefined> => {
  maybeThrow("get-many")
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
  maybeThrow("set-many")
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
  maybeThrow("delete-many")
  const map = __getBackingMap(bucket)
  for (const k of keys) map.delete(k)
}

export const keys = (bucket: Bucket): Array<string> => {
  maybeThrow("keys")
  return Array.from(__getBackingMap(bucket).keys())
}
