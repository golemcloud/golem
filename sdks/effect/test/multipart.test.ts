import { describe, expect, it } from "@effect/vitest"
import { vi } from "vitest"
import { decodeMultipart, encodeMultipart, extractBoundary } from "../src/internal/multipart.js"

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
    const colliding = "11111111-1111-1111-1111-111111111111"
    const safe = "22222222-2222-2222-2222-222222222222"
    const randomUUID = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValueOnce(colliding)
      .mockReturnValue(safe)
    try {
      const marker = `\r\n--${colliding.replace(/-/g, "")}`
      const body = enc(`\r\r\n--almost\r\n--111x${marker}`)
      const { data, boundary } = encodeMultipart([
        { name: "db", contentType: "application/x-sqlite3", body },
      ])
      expect(boundary).toBe(safe.replace(/-/g, ""))
      expect(decodeMultipart(data, boundary)[0]!.body).toEqual(body)
    } finally {
      randomUUID.mockRestore()
    }
  })

  it("round-trips multi-MB binary parts between JSON parts without aliasing the input", () => {
    const body = new Uint8Array(4 * 1024 * 1024)
    for (let i = 0; i < body.length; i++) body[i] = i % 256
    const parts = [
      { name: "state", contentType: "application/json", body: enc('{"count":17}') },
      { name: "db", contentType: "application/x-sqlite3", body },
      { name: "tail", contentType: "application/json", body: enc('{"last":true}') },
    ]
    const { data, boundary } = encodeMultipart(parts)
    const decoded = decodeMultipart(data, boundary)
    expect(decoded.map((part) => part.name)).toEqual(["state", "db", "tail"])
    expect(decoded.map((part) => part.contentType)).toEqual(parts.map((part) => part.contentType))
    for (let i = 0; i < parts.length; i++) {
      expect(Buffer.from(decoded[i]!.body).equals(Buffer.from(parts[i]!.body))).toBe(true)
    }
    data.fill(0)
    expect(Buffer.from(decoded[1]!.body).equals(Buffer.from(body))).toBe(true)
  })

  it.each(["\r\n", "\n"])(
    "preserves binary data with %j framing and false delimiter starts",
    (newline) => {
      const boundary = "abcdef0123456789abcdef0123456789"
      const body = `\n\n--almost\n--${boundary.slice(0, -1)}x\r\n\r\nend\n`
      const raw =
        `--${boundary}${newline}` +
        `Content-Type: application/octet-stream${newline}` +
        `Content-Disposition: attachment; name="db"${newline}${newline}` +
        `${body}${newline}--${boundary}--${newline}`
      expect(decodeMultipart(enc(raw), boundary)[0]!.body).toEqual(enc(body))
    },
  )

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
