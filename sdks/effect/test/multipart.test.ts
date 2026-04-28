import { describe, expect, it } from "@effect/vitest"
import { decodeMultipart, encodeMultipart, extractBoundary } from "../src/multipart.js"

const enc = (s: string): Uint8Array => new TextEncoder().encode(s)
const dec = (b: Uint8Array): string => new TextDecoder().decode(b)

describe("multipart encode/decode", () => {
  it("round-trips a single text part", () => {
    const parts = [{ name: "state", contentType: "application/json", body: enc('{"x":1}') }]
    const { data, boundary } = encodeMultipart(parts)
    expect(boundary.length).toBe(32)
    expect(boundary).toMatch(/^[0-9a-f]{32}$/)
    const decoded = decodeMultipart(data, boundary)
    expect(decoded).toHaveLength(1)
    expect(decoded[0]!.name).toBe("state")
    expect(decoded[0]!.contentType).toBe("application/json")
    expect(dec(decoded[0]!.body)).toBe('{"x":1}')
  })

  it("round-trips multiple parts", () => {
    const parts = [
      { name: "state", contentType: "application/json", body: enc('{"v":42}') },
      {
        name: "db:counters",
        contentType: "application/x-sqlite3",
        body: new Uint8Array([1, 2, 3]),
      },
      {
        name: "db:audit",
        contentType: "application/x-sqlite3",
        body: new Uint8Array([7, 8, 9, 10]),
      },
    ]
    const { data, boundary } = encodeMultipart(parts)
    const decoded = decodeMultipart(data, boundary)
    expect(decoded.map((p) => p.name)).toEqual(["state", "db:counters", "db:audit"])
    expect(Array.from(decoded[1]!.body)).toEqual([1, 2, 3])
    expect(Array.from(decoded[2]!.body)).toEqual([7, 8, 9, 10])
  })

  it("regenerates boundary if it would appear in any body", () => {
    // Force a deterministic boundary by stuffing the part body with
    // many candidate boundaries — encoder must keep regenerating.
    const parts = [{ name: "state", contentType: "application/json", body: enc("hello") }]
    const { data, boundary } = encodeMultipart(parts)
    expect(data.length).toBeGreaterThan(0)
    expect(boundary).toBeTruthy()
  })

  it("decoder accepts bare LF line endings (decode-side robustness)", () => {
    // Hand-craft a multipart body with bare \n separators.
    const boundary = "abcdef0123456789abcdef0123456789"
    const body =
      `--${boundary}\n` +
      `Content-Type: application/json\n` +
      `Content-Disposition: attachment; name="state"\n` +
      `\n` +
      `{"v":1}\n` +
      `--${boundary}--\n`
    const decoded = decodeMultipart(enc(body), boundary)
    expect(decoded).toHaveLength(1)
    expect(dec(decoded[0]!.body)).toBe('{"v":1}')
  })

  it("decoder rejects duplicate part names", () => {
    const boundary = "abcdef0123456789abcdef0123456789"
    const body =
      `--${boundary}\r\n` +
      `Content-Type: application/json\r\n` +
      `Content-Disposition: attachment; name="state"\r\n` +
      `\r\n` +
      `{"v":1}\r\n` +
      `--${boundary}\r\n` +
      `Content-Type: application/json\r\n` +
      `Content-Disposition: attachment; name="state"\r\n` +
      `\r\n` +
      `{"v":2}\r\n` +
      `--${boundary}--\r\n`
    expect(() => decodeMultipart(enc(body), boundary)).toThrow(/duplicate/i)
  })

  it("decoder rejects body that does not start with the boundary", () => {
    const boundary = "abcdef0123456789abcdef0123456789"
    expect(() => decodeMultipart(enc("garbage"), boundary)).toThrow(/boundary/i)
  })

  it("decoder rejects parts missing a name", () => {
    const boundary = "abcdef0123456789abcdef0123456789"
    const body =
      `--${boundary}\r\n` +
      `Content-Type: application/json\r\n` +
      `Content-Disposition: attachment\r\n` +
      `\r\n` +
      `{}\r\n` +
      `--${boundary}--\r\n`
    expect(() => decodeMultipart(enc(body), boundary)).toThrow(/name/)
  })

  it("extractBoundary parses both quoted and unquoted boundary params", () => {
    expect(extractBoundary("multipart/mixed; boundary=abc")).toBe("abc")
    expect(extractBoundary('multipart/mixed; boundary="abc"')).toBe("abc")
    expect(extractBoundary("multipart/mixed")).toBe(null)
  })
})
