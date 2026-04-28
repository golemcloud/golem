/**
 * Vitest mock for `wasi:keyvalue/eventual@0.1.0`.
 */

import { Bucket, IncomingValue, OutgoingValue, __getBackingMap } from "./wasi-keyvalue-types.js"

let __nextError: { op: "get" | "set" | "delete" | "exists"; trace: string } | undefined

export const __setNextError = (op: "get" | "set" | "delete" | "exists", trace: string): void => {
  __nextError = { op, trace }
}
export const __resetNextError = (): void => {
  __nextError = undefined
}

const maybeThrow = (op: "get" | "set" | "delete" | "exists"): void => {
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

export const get = (bucket: Bucket, key: string): IncomingValue | undefined => {
  maybeThrow("get")
  const bytes = __getBackingMap(bucket).get(key)
  if (bytes === undefined) return undefined
  return new IncomingValue(bytes)
}

export const set = (bucket: Bucket, key: string, ov: OutgoingValue): void => {
  maybeThrow("set")
  const bytes = (ov as { __bytes?: Uint8Array }).__bytes
  if (bytes === undefined) {
    throw new Error("OutgoingValue body not set before eventual.set")
  }
  __getBackingMap(bucket).set(key, bytes)
}

export const delete_ = (bucket: Bucket, key: string): void => {
  maybeThrow("delete")
  __getBackingMap(bucket).delete(key)
}

export const exists = (bucket: Bucket, key: string): boolean => {
  maybeThrow("exists")
  return __getBackingMap(bucket).has(key)
}
