/**
 * Runtime mock for `golem:api/host@1.5.0`. Only the bits actually used
 * by `src/client.ts` are implemented.
 */

import { uuidToString, type Uuid } from "./golem-core-types.js"

let counter = 0n

export const __resetIdempotency = (): void => {
  counter = 0n
}

/** Returns a deterministic, unique UUID per call (counter-based). */
export const generateIdempotencyKey = (): Uuid => {
  counter += 1n
  return { highBits: 0n, lowBits: counter }
}

export const __nextIdempotencyKeyAsString = (): string => uuidToString(generateIdempotencyKey())
