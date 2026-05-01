import { describe, expect, it } from "@effect/vitest"
import {
  decodeEnvelope,
  encodeBinaryEnvelope,
  encodeJsonEnvelope,
  SnapshotEnvelopeError,
  UnsupportedSnapshotFormatError,
} from "../src/SnapshotEnvelope.js"

const anonymous = { tag: "anonymous" } as const
const oidcAlice = {
  tag: "oidc",
  val: { sub: "alice", issuer: "https://example.test", claims: "{}" },
} as const

describe("snapshot-envelope JSON format", () => {
  it("round-trips an envelope with primitive state", () => {
    const env = encodeJsonEnvelope(anonymous, { count: 42 })
    expect(env.mimeType).toBe("application/json")
    const text = new TextDecoder().decode(env.payload)
    const obj = JSON.parse(text)
    expect(obj).toEqual({
      version: 1,
      principal: anonymous,
      state: { count: 42 },
    })
    const decoded = decodeEnvelope(env, anonymous)
    expect(decoded).toEqual({ kind: "json", principal: anonymous, state: { count: 42 } })
  })

  it("preserves the embedded principal on decode", () => {
    const env = encodeJsonEnvelope(oidcAlice, "x")
    const decoded = decodeEnvelope(env, anonymous)
    expect(decoded.kind).toBe("json")
    if (decoded.kind !== "json") throw new Error()
    expect(decoded.principal).toEqual(oidcAlice)
  })

  it("rejects malformed JSON payloads", () => {
    expect(() =>
      decodeEnvelope(
        { payload: new TextEncoder().encode("not json"), mimeType: "application/json" },
        anonymous,
      ),
    ).toThrow(SnapshotEnvelopeError)
  })

  it("rejects unexpected envelope versions", () => {
    const bad = new TextEncoder().encode(
      JSON.stringify({ version: 99, principal: anonymous, state: 0 }),
    )
    expect(() => decodeEnvelope({ payload: bad, mimeType: "application/json" }, anonymous)).toThrow(
      SnapshotEnvelopeError,
    )
  })

  it("rejects payloads missing required fields", () => {
    const bad = new TextEncoder().encode(JSON.stringify({ version: 1, state: 0 }))
    expect(() => decodeEnvelope({ payload: bad, mimeType: "application/json" }, anonymous)).toThrow(
      SnapshotEnvelopeError,
    )
  })
})

describe("snapshot-envelope binary v2 format", () => {
  it("round-trips a user payload + principal", () => {
    const userBytes = new Uint8Array([1, 2, 3, 4, 5])
    const env = encodeBinaryEnvelope(oidcAlice, userBytes)
    expect(env.mimeType).toBe("application/octet-stream")
    expect(env.payload[0]).toBe(2) // version byte
    const decoded = decodeEnvelope(env, anonymous)
    expect(decoded.kind).toBe("binary")
    if (decoded.kind !== "binary") throw new Error()
    expect(decoded.principal).toEqual(oidcAlice)
    expect(Array.from(decoded.userPayload)).toEqual([1, 2, 3, 4, 5])
  })

  it("uses big-endian length encoding", () => {
    const userBytes = new Uint8Array(0)
    const env = encodeBinaryEnvelope(anonymous, userBytes)
    const view = new DataView(env.payload.buffer, env.payload.byteOffset, env.payload.byteLength)
    const principalLen = view.getUint32(1, false)
    const expected = new TextEncoder().encode(JSON.stringify(anonymous)).length
    expect(principalLen).toBe(expected)
  })

  it("decodes a legacy v1 binary envelope using the fallback principal", () => {
    // v1 layout: byte 0 = 1, then raw user bytes.
    const userBytes = new Uint8Array([7, 8, 9])
    const payload = new Uint8Array(1 + userBytes.length)
    payload[0] = 1
    payload.set(userBytes, 1)
    const decoded = decodeEnvelope({ payload, mimeType: "application/octet-stream" }, oidcAlice)
    if (decoded.kind !== "binary") throw new Error()
    expect(decoded.principal).toEqual(oidcAlice)
    expect(Array.from(decoded.userPayload)).toEqual([7, 8, 9])
  })

  it("rejects truncated v2 headers", () => {
    const payload = new Uint8Array([2, 0, 0]) // version=2 but truncated len
    expect(() =>
      decodeEnvelope({ payload, mimeType: "application/octet-stream" }, anonymous),
    ).toThrow(SnapshotEnvelopeError)
  })

  it("rejects v2 payloads where principalLen exceeds the buffer", () => {
    const payload = new Uint8Array(5)
    payload[0] = 2
    new DataView(payload.buffer).setUint32(1, 0xffff_ffff, false)
    expect(() =>
      decodeEnvelope({ payload, mimeType: "application/octet-stream" }, anonymous),
    ).toThrow(SnapshotEnvelopeError)
  })

  it("rejects unsupported version bytes", () => {
    const payload = new Uint8Array([99, 0, 0, 0, 0])
    expect(() =>
      decodeEnvelope({ payload, mimeType: "application/octet-stream" }, anonymous),
    ).toThrow(SnapshotEnvelopeError)
  })
})

describe("snapshot-envelope unsupported formats", () => {
  it("rejects malformed multipart/mixed envelopes (no body)", () => {
    expect(() =>
      decodeEnvelope(
        {
          payload: new Uint8Array(0),
          mimeType: "multipart/mixed; boundary=abc",
        },
        anonymous,
      ),
    ).toThrow(SnapshotEnvelopeError)
  })

  it("rejects multipart/mixed without a boundary parameter", () => {
    expect(() =>
      decodeEnvelope(
        {
          payload: new Uint8Array(0),
          mimeType: "multipart/mixed",
        },
        anonymous,
      ),
    ).toThrow(UnsupportedSnapshotFormatError)
  })

  it("rejects unknown mime types", () => {
    expect(() =>
      decodeEnvelope({ payload: new Uint8Array(0), mimeType: "text/plain" }, anonymous),
    ).toThrow(UnsupportedSnapshotFormatError)
  })
})
