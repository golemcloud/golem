import type { JsonValue } from "../../SchemaRef.js"

/** Reject values that JSON serialization would discard or coerce. */
export function assertJsonValue(value: unknown): asserts value is JsonValue {
  stringifyJsonValue(value)
}

/** Serialize data properties without evaluating getters or serialization hooks. */
export function stringifyJsonValue(value: unknown): string {
  const ancestors = new Set<object>()
  function visit(value: unknown): string {
    if (typeof value === "string" && /[\uD800-\uDFFF]/u.test(value))
      throw new TypeError("JSON strings cannot contain unpaired surrogates")
    if (value === null || typeof value === "string" || typeof value === "boolean")
      return JSON.stringify(value)
    if (typeof value === "number" && Number.isFinite(value)) return JSON.stringify(value)
    if (typeof value !== "object" || value === null) throw new TypeError("Expected a JSON value")
    if ("toJSON" in value && !Object.prototype.hasOwnProperty.call(value, "toJSON"))
      throw new TypeError("JSON values cannot inherit serialization hooks")
    if (ancestors.has(value)) throw new TypeError("JSON values cannot contain cycles")
    const array = Array.isArray(value)
    const prototype = Object.getPrototypeOf(value)
    if (array && prototype !== Array.prototype)
      throw new TypeError("JSON arrays must have the standard array prototype")
    if (!array && prototype !== Object.prototype && prototype !== null)
      throw new TypeError("JSON objects must be plain objects")
    ancestors.add(value)
    try {
      const length = array ? Object.getOwnPropertyDescriptor(value, "length")!.value : 0
      const keys = Reflect.ownKeys(value).filter((key) => !(array && key === "length"))
      if (array && keys.length !== length)
        throw new TypeError("JSON arrays must be dense and have no extra properties")
      const parts: string[] = []
      for (const key of keys) {
        if (
          typeof key !== "string" ||
          (array && (!/^(0|[1-9][0-9]*)$/.test(key) || Number(key) >= length))
        )
          throw new TypeError("JSON values cannot contain symbol keys or extra array properties")
        const descriptor = Object.getOwnPropertyDescriptor(value, key)!
        if (!descriptor.enumerable || !("value" in descriptor))
          throw new TypeError("JSON properties must be enumerable data properties")
        const encoded = visit(descriptor.value)
        if (array) parts[Number(key)] = encoded
        else parts.push(`${visit(key)}:${encoded}`)
      }
      return array ? `[${parts.join(",")}]` : `{${parts.join(",")}}`
    } finally {
      ancestors.delete(value)
    }
  }
  return visit(value)
}

export function parseJsonValue(text: string): JsonValue {
  if (typeof text !== "string") throw new TypeError("JSON wire values must contain text")
  const value: unknown = JSON.parse(text)
  assertJsonValue(value)
  return value
}
