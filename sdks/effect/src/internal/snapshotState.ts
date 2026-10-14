import { Predicate } from "effect"
import { SnapshotEnvelopeError } from "./snapshotEnvelope.js"

const marker = "$golemSnapshot"
const decimal = /^(?:0|-?[1-9][0-9]*)$/

/** Encode only schema-encoded state; envelope metadata and binary parts are not state. */
export const encodeSnapshotState = (value: unknown): unknown => {
  if (Predicate.isBigInt(value)) return { [marker]: ["bigint", value.toString()] }
  if (Array.isArray(value)) return value.map(encodeSnapshotState)
  if (Predicate.isObject(value)) {
    const prototype = Object.getPrototypeOf(value)
    if (prototype !== Object.prototype && prototype !== null) return value
    const entries = Object.entries(value).map(([key, item]) => [key, encodeSnapshotState(item)])
    return Object.hasOwn(value, marker)
      ? { [marker]: ["object", entries] }
      : Object.fromEntries(entries)
  }
  return value
}

export const decodeSnapshotState = (value: unknown): unknown => {
  if (Array.isArray(value)) return value.map(decodeSnapshotState)
  if (!Predicate.isObject(value)) return value
  if (Object.hasOwn(value, marker) && Object.keys(value).length === 1) {
    const tag = value[marker]
    if (Array.isArray(tag) && tag.length === 2) {
      if (
        tag[0] === "bigint" &&
        Predicate.isString(tag[1]) &&
        decimal.exec(tag[1])?.[0] === tag[1]
      ) {
        return BigInt(tag[1])
      }
      if (tag[0] === "object" && Array.isArray(tag[1])) {
        const keys = new Set<string>()
        return Object.fromEntries(
          tag[1].map((entry: unknown) => {
            if (
              !Array.isArray(entry) ||
              entry.length !== 2 ||
              !Predicate.isString(entry[0]) ||
              keys.has(entry[0])
            ) {
              throw new SnapshotEnvelopeError("invalid snapshot state object entry")
            }
            keys.add(entry[0])
            return [entry[0], decodeSnapshotState(entry[1])]
          }),
        )
      }
    }
    throw new SnapshotEnvelopeError("invalid snapshot state tag")
  }
  return Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, decodeSnapshotState(item)]),
  )
}
