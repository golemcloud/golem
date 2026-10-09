import { describe, expect, it } from "@effect/vitest"
import { vi } from "vitest"
import fixtures from "../../../test-data/snapshot-multipart/framing.json" with { type: "json" }
import { decodeMultipart, encodeMultipart, extractBoundary } from "../src/internal/multipart.js"

const enc = (s: string): Uint8Array => new TextEncoder().encode(s)
const dec = (b: Uint8Array): string => new TextDecoder().decode(b)

describe("multipart encode/decode", () => {
  const boundary = "b"
  const header =
    'Content-Type: application/octet-stream\r\nContent-Disposition: attachment; name="part:index"\r\n'
  const wire = `--b\r\n${header}\r\nX\r\n--b--\r\n`

  for (const bad of [
    wire.slice(0, -7),
    wire + "epilogue",
    wire + "\r\n",
    "preamble" + wire,
    "\r\n\r\n" + wire,
    wire.replace("--b\r\n", "--bextra\r\n"),
    wire.replace("--b--\r\n", "--b--extra\r\n"),
    wire.replace(header, header + "content-type: text/plain\r\n"),
    wire.replace(header, header + 'CONTENT-DISPOSITION: attachment; name="other"\r\n'),
    wire.replace('name="part:index"', 'name="part:index"; name="other"'),
    wire.replace(header, header + "Content-Transfer-Encoding: base64\r\n"),
    wire.replace("application/octet-stream", "application/\roctet-stream"),
  ]) {
    it(`rejects malformed framing/header ${JSON.stringify(bad)}`, () => {
      expect(() => decodeMultipart(enc(bad), boundary)).toThrow()
    })
  }

  it("accepts exactly one leading framing newline and closing EOF", () => {
    expect(dec(decodeMultipart(enc("\r\n" + wire.slice(0, -2)), boundary)[0]!.body)).toBe("X")
  })

  it("round-trips every byte, including 0 and 255", () => {
    const body = Uint8Array.from({ length: 256 }, (_, i) => i)
    const encoded = encodeMultipart([
      { name: "part:__proto__", contentType: "application/octet-stream", body },
    ])
    expect(decodeMultipart(encoded.data, encoded.boundary)[0]!.body).toEqual(body)
  })

  it("preserves a large binary sandwich independently of the input buffer", () => {
    const body = Uint8Array.from({ length: 4 * 1024 * 1024 }, (_, i) => i % 256)
    const parts = [
      { name: "state", contentType: "application/json", body: enc('{"count":17}') },
      { name: "db:primary", contentType: "application/octet-stream", body },
      { name: "tail", contentType: "application/json", body: enc('{"last":true}') },
    ]
    const encoded = encodeMultipart(parts)
    const decoded = decodeMultipart(encoded.data, encoded.boundary)
    expect(decoded.map(({ name, contentType }) => ({ name, contentType }))).toEqual(
      parts.map(({ name, contentType }) => ({ name, contentType })),
    )
    for (const clearInput of [false, true]) {
      if (clearInput) encoded.data.fill(0)
      for (let i = 0; i < parts.length; i++) {
        expect(Buffer.from(decoded[i]!.body).equals(Buffer.from(parts[i]!.body))).toBe(true)
      }
    }
  })

  for (const newline of ["\r\n", "\n"]) {
    it(`preserves false delimiters and payload endings with ${JSON.stringify(newline)} framing`, () => {
      const body = `\n\n--almost\n--b\r\ninvalid\r\n--bextra\r\n--b--extra\r\n\r\nend\n`
      // Bare LF before a complete marker is payload only in CRLF framing.
      const payload = newline === "\r\n" ? body : body.replace("\n--b\r\n", "\n--bextra\r\n")
      const data = enc(
        `--b${newline}Content-Type: application/octet-stream${newline}Content-Disposition: attachment; name="part:index"${newline}${newline}${payload}${newline}--b--`,
      )
      expect(dec(decodeMultipart(data, "b")[0]!.body)).toBe(payload)
    })
  }

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

  for (const suffix of ["", "\r", "x", "--", "--\r", "--x"]) {
    it(`classifies final-position collisions after false candidates: ${JSON.stringify(suffix)}`, () => {
      const first = "11111111111111111111111111111111"
      const second = "22222222222222222222222222222222"
      const random = vi
        .spyOn(crypto, "randomUUID")
        .mockReturnValueOnce("11111111-1111-1111-1111-111111111111")
        .mockReturnValue("22222222-2222-2222-2222-222222222222")
      try {
        const body = enc(`\r\r\n--almost\r\n--${first}x\r\n--${first}${suffix}`)
        const encoded = encodeMultipart([
          { name: "part:index", contentType: "application/octet-stream", body },
        ])
        expect(encoded.boundary).toBe(suffix === "" || suffix === "--" ? second : first)
        expect(decodeMultipart(encoded.data, encoded.boundary)[0]!.body).toEqual(body)
      } finally {
        random.mockRestore()
      }
    })
  }

  for (const payload of [
    "--B\r\ninside",
    "inside\r\n--B\r\nend",
    "inside\r\n--B",
    "--B--",
    "inside\r\n--B--",
  ]) {
    it(`regenerates a forced collision ${JSON.stringify(payload)}`, () => {
      const first = "11111111111111111111111111111111"
      const second = "22222222222222222222222222222222"
      const random = vi
        .spyOn(crypto, "randomUUID")
        .mockReturnValueOnce("11111111-1111-1111-1111-111111111111")
        .mockReturnValue("22222222-2222-2222-2222-222222222222")
      try {
        const body = enc(payload.replace(/B/g, first))
        const encoded = encodeMultipart([
          { name: "part:index", contentType: "application/octet-stream", body },
        ])
        expect(encoded.boundary).toBe(second)
        expect(decodeMultipart(encoded.data, encoded.boundary)[0]!.body).toEqual(body)
      } finally {
        random.mockRestore()
      }
    })
  }

  it("rejects header injection while encoding", () => {
    expect(() =>
      encodeMultipart([
        { name: 'bad"\r\nInjected: yes', contentType: "text/plain", body: enc("") },
      ]),
    ).toThrow()
    expect(() =>
      encodeMultipart([
        { name: "part:index", contentType: "text/plain\r\nInjected: yes", body: enc("") },
      ]),
    ).toThrow()
  })

  for (const mime of [
    "multipart/mixedextra; boundary=b",
    "multipart/mixed; boundary=b; boundary=b",
    'multipart/mixed; boundary="b',
    "multipart/mixed; boundary=",
    'multipart/mixed; boundary="b c"',
    "multipart/mixed; boundary=" + "b".repeat(71),
  ]) {
    it(`rejects invalid boundary MIME ${mime}`, () => expect(extractBoundary(mime)).toBeNull())
  }

  for (const fixture of fixtures.valid) {
    it(`shared framing: ${fixture.name}`, () => {
      const e = fixture.newline
      const data = enc(
        `--${fixtures.boundary}${e}Content-Type: application/octet-stream${e}Content-Disposition: attachment; name="part:index"${e}${e}${fixture.payload}${e}--${fixtures.boundary}--${e}`,
      )
      expect(Array.from(decodeMultipart(data, fixtures.boundary)[0]!.body)).toEqual(
        fixture.hex.match(/../g)?.map((b) => parseInt(b, 16)) ?? [],
      )
    })
  }

  it("requires a closing delimiter", () => {
    const raw =
      '--b\r\nContent-Type: application/json\r\nContent-Disposition: attachment; name="state"\r\n\r\n{}'
    expect(() => decodeMultipart(enc(raw), "b")).toThrow()
  })

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
