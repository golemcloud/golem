import { describe, expect, it } from "@effect/vitest"
import {
  decodeEnvelope,
  encodeBinaryEnvelope,
  encodeJsonEnvelope,
  SnapshotEnvelopeError,
  UnsupportedSnapshotFormatError,
} from "../src/internal/snapshotEnvelope.js"

// The 1.6 principal of a snapshot, in the JSON shape that 1.6 wrote.
const anonymous = { tag: "anonymous" } as const
const oidcAlice = {
  tag: "oidc",
  val: { sub: "alice", issuer: "https://example.test", claims: "{}" },
} as const

const text = (value: unknown) => new TextEncoder().encode(JSON.stringify(value))

/** A 1.6 binary envelope: `0x02`, the principal length, the principal JSON, then the state. */
const legacyBinary = (principal: unknown, userBytes: Uint8Array): Uint8Array => {
  const principalBytes = text(principal)
  const out = new Uint8Array(5 + principalBytes.length + userBytes.length)
  out[0] = 2
  new DataView(out.buffer).setUint32(1, principalBytes.length, false)
  out.set(principalBytes, 5)
  out.set(userBytes, 5 + principalBytes.length)
  return out
}

describe("snapshot-envelope JSON format", () => {
  it("writes the 2.0 envelope with no principal", () => {
    const env = encodeJsonEnvelope({ count: 42 })
    expect(env.mimeType).toBe("application/json")
    expect(JSON.parse(new TextDecoder().decode(env.payload))).toEqual({
      version: 2,
      state: { count: 42 },
    })
    expect(decodeEnvelope(env)).toEqual({ kind: "json", state: { count: 42 } })
  })

  it("refuses a 1.6 envelope as an unknown version", () => {
    for (const principal of [anonymous, oidcAlice]) {
      expect(() =>
        decodeEnvelope({
          payload: text({ version: 1, principal, state: "x" }),
          mimeType: "application/json",
        }),
      ).toThrow(SnapshotEnvelopeError)
    }
  })

  it("rejects malformed JSON payloads", () => {
    expect(() =>
      decodeEnvelope({
        payload: new TextEncoder().encode("not json"),
        mimeType: "application/json",
      }),
    ).toThrow(SnapshotEnvelopeError)
  })

  it("rejects unexpected envelope versions", () => {
    for (const version of [0, 3, 99]) {
      expect(() =>
        decodeEnvelope({
          payload: text({ version, state: 0 }),
          mimeType: "application/json",
        }),
      ).toThrow(SnapshotEnvelopeError)
    }
  })

  it("rejects payloads missing required fields", () => {
    expect(() =>
      decodeEnvelope({ payload: text({ version: 2 }), mimeType: "application/json" }),
    ).toThrow(SnapshotEnvelopeError)
  })
})

describe("snapshot-envelope binary format", () => {
  it("writes the version byte 3 and the user payload", () => {
    const env = encodeBinaryEnvelope(new Uint8Array([1, 2, 3, 4, 5]))
    expect(env.mimeType).toBe("application/octet-stream")
    expect(Array.from(env.payload)).toEqual([3, 1, 2, 3, 4, 5])
    const decoded = decodeEnvelope(env)
    expect(decoded.kind).toBe("binary")
    if (decoded.kind !== "binary") throw new Error()
    expect(Array.from(decoded.userPayload)).toEqual([1, 2, 3, 4, 5])
  })

  it("round-trips an empty user payload", () => {
    const decoded = decodeEnvelope(encodeBinaryEnvelope(new Uint8Array(0)))
    expect(decoded.kind).toBe("binary")
    if (decoded.kind !== "binary") throw new Error()
    expect(decoded.userPayload.length).toBe(0)
  })

  it("refuses a 1.6 binary v2 envelope as an unknown version", () => {
    for (const principal of [anonymous, oidcAlice]) {
      let refusal: unknown
      try {
        decodeEnvelope({
          payload: legacyBinary(principal, new Uint8Array([7, 8, 9])),
          mimeType: "application/octet-stream",
        })
      } catch (error) {
        refusal = error
      }
      expect(refusal).toBeInstanceOf(SnapshotEnvelopeError)
      expect((refusal as SnapshotEnvelopeError).reason).toContain("unsupported version byte 2")
    }
  })

  it("rejects the removed v1 binary envelope", () => {
    const userBytes = new Uint8Array([7, 8, 9])
    const payload = new Uint8Array(1 + userBytes.length)
    payload[0] = 1
    payload.set(userBytes, 1)
    expect(() => decodeEnvelope({ payload, mimeType: "application/octet-stream" })).toThrow(
      SnapshotEnvelopeError,
    )
  })

  it("rejects unsupported version bytes", () => {
    const payload = new Uint8Array([99, 0, 0, 0, 0])
    expect(() => decodeEnvelope({ payload, mimeType: "application/octet-stream" })).toThrow(
      SnapshotEnvelopeError,
    )
  })
})

describe("snapshot-envelope unsupported formats", () => {
  it("rejects malformed multipart/mixed envelopes (no body)", () => {
    expect(() =>
      decodeEnvelope({
        payload: new Uint8Array(0),
        mimeType: "multipart/mixed; boundary=abc",
      }),
    ).toThrow(SnapshotEnvelopeError)
  })

  it("rejects multipart/mixed without a boundary parameter", () => {
    expect(() =>
      decodeEnvelope({
        payload: new Uint8Array(0),
        mimeType: "multipart/mixed",
      }),
    ).toThrow(UnsupportedSnapshotFormatError)
  })

  it("rejects unknown mime types", () => {
    expect(() => decodeEnvelope({ payload: new Uint8Array(0), mimeType: "text/plain" })).toThrow(
      UnsupportedSnapshotFormatError,
    )
  })
})
