import { describe, expect, it } from "@effect/vitest"
import * as fc from "effect/testing/FastCheck"
import { decodeMultipart, encodeMultipart, extractBoundary } from "../src/internal/multipart.js"

// ---------------------------------------------------------------------------
// Arbitraries
// ---------------------------------------------------------------------------

/**
 * Multipart part **names** must:
 * - be non-empty (decoder rejects empty names),
 * - not contain a double-quote (encoder writes `name="<name>"` and the
 *   decoder regex is `name="([^"]+)"`),
 * - not contain CR/LF (the decoder splits headers on `\r?\n`).
 *
 * We pick the safe ASCII subset `[A-Za-z0-9_\-]+` to avoid having to
 * reason about every code point.
 */
const partNameArb = fc.stringMatching(/^[A-Za-z0-9_-]+$/).filter((s) => s.length > 0)

/**
 * Content-Type header values must not contain CR/LF (they're parsed
 * line-by-line) and are surface-trimmed on decode — so we generate
 * MIME-like ASCII tokens.
 */
const contentTypeArb = fc.stringMatching(/^[A-Za-z0-9./+_-]+$/).filter((s) => s.length > 0)

const partArb = fc.record({
  name: partNameArb,
  contentType: contentTypeArb,
  body: fc.uint8Array({ maxLength: 256 }),
})

/** De-duplicate by `name` — the decoder rejects duplicate names. */
const partsArb = fc
  .uniqueArray(partArb, { maxLength: 4, selector: (p) => p.name })
  .filter((arr) => arr.length > 0)

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

describe("multipart codec properties", () => {
  it.prop(
    "encodeMultipart -> decodeMultipart round-trips arbitrary parts",
    { parts: partsArb },
    ({ parts }) => {
      const { data, boundary } = encodeMultipart(parts)
      const decoded = decodeMultipart(data, boundary)
      expect(decoded.length).toBe(parts.length)
      for (let i = 0; i < parts.length; i++) {
        expect(decoded[i]!.name).toBe(parts[i]!.name)
        expect(decoded[i]!.contentType).toBe(parts[i]!.contentType)
        expect(Array.from(decoded[i]!.body)).toEqual(Array.from(parts[i]!.body))
      }
    },
  )

  it.prop(
    "the chosen boundary never appears as `\\r\\n--<boundary>` in any part body",
    { parts: partsArb },
    ({ parts }) => {
      const { data, boundary } = encodeMultipart(parts)
      // Sanity: the boundary itself is a 32-char lowercase hex string.
      expect(boundary).toMatch(/^[0-9a-f]{32}$/)
      // The encoder regenerates the boundary if `\r\n--<boundary>`
      // appears in any body. Verify by re-parsing only the body region
      // of each part and asserting the marker is absent.
      const marker = new TextEncoder().encode(`\r\n--${boundary}`)
      for (const part of parts) {
        expect(indexOf(part.body, marker)).toBe(-1)
      }
      // And of course the round-trip must still work.
      const decoded = decodeMultipart(data, boundary)
      expect(decoded.length).toBe(parts.length)
    },
  )

  it.prop(
    "extractBoundary recovers the boundary embedded in the mime type",
    { parts: partsArb },
    ({ parts }) => {
      const { boundary } = encodeMultipart(parts)
      const mime = `multipart/mixed; boundary=${boundary}`
      expect(extractBoundary(mime)).toBe(boundary)
    },
  )
})

/** Local re-implementation of `Uint8Array.indexOf(subarray)`. */
function indexOf(haystack: Uint8Array, needle: Uint8Array): number {
  if (needle.length === 0) return 0
  outer: for (let i = 0; i <= haystack.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) {
      if (haystack[i + j] !== needle[j]) continue outer
    }
    return i
  }
  return -1
}
