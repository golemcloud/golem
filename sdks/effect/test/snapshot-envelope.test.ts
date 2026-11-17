import { describe, expect, it } from "@effect/vitest"
import type * as CoreTypes from "golem:core/types@2.0.0"
import {
  decodeConstructorParameters,
  decodeEnvelope,
  encodeBinaryEnvelope,
  encodeConstructorParameters,
  encodeJsonEnvelope,
  SnapshotEnvelopeError,
  UnsupportedSnapshotFormatError,
} from "../src/internal/snapshotEnvelope.js"

const anonymous = { tag: "anonymous" } as const
const oidcAlice = {
  tag: "oidc",
  val: { sub: "alice", issuer: "https://example.test", claims: "{}" },
} as const
const constructorParameters: CoreTypes.SchemaValueTree = {
  root: 0,
  valueNodes: [
    { tag: "record-value", val: [1, 2, 3] },
    { tag: "string-value", val: "alice" },
    { tag: "u64-value", val: 9007199254740993n },
    { tag: "binary-value", val: { tag: "inline", val: { data: new Uint8Array([0, 255]) } } },
  ] as CoreTypes.SchemaValueNode[],
}
const header = (principal: typeof anonymous | typeof oidcAlice = anonymous) => ({
  principal,
  constructorParameters,
})

describe("snapshot-envelope JSON format", () => {
  it("round-trips an envelope with primitive state and the constructor parameters", () => {
    const env = encodeJsonEnvelope(header(), { count: 42 })
    expect(env.mimeType).toBe("application/json")
    const text = new TextDecoder().decode(env.payload)
    const obj = JSON.parse(text)
    expect(obj).toMatchObject({ version: 1, principal: anonymous, state: { count: 42 } })
    expect(obj.constructorParameters.valueNodes[2]).toEqual({
      tag: "u64-value",
      val: { $bigint: "9007199254740993" },
    })
    const decoded = decodeEnvelope(env)
    expect(decoded).toEqual({
      kind: "json",
      principal: anonymous,
      constructorParameters,
      state: { count: 42 },
    })
  })

  it("preserves the embedded principal on decode", () => {
    const env = encodeJsonEnvelope(header(oidcAlice), "x")
    const decoded = decodeEnvelope(env)
    expect(decoded.kind).toBe("json")
    if (decoded.kind !== "json") throw new Error()
    expect(decoded.principal).toEqual(oidcAlice)
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
    const bad = new TextEncoder().encode(
      JSON.stringify({ version: 99, principal: anonymous, constructorParameters: {}, state: 0 }),
    )
    expect(() => decodeEnvelope({ payload: bad, mimeType: "application/json" })).toThrow(
      SnapshotEnvelopeError,
    )
  })

  it("rejects payloads missing required fields", () => {
    const bad = new TextEncoder().encode(JSON.stringify({ version: 1, state: 0 }))
    expect(() => decodeEnvelope({ payload: bad, mimeType: "application/json" })).toThrow(
      SnapshotEnvelopeError,
    )
  })

  it("rejects an envelope without constructor parameters", () => {
    const bad = new TextEncoder().encode(
      JSON.stringify({ version: 1, principal: anonymous, state: 0 }),
    )
    expect(() => decodeEnvelope({ payload: bad, mimeType: "application/json" })).toThrow(
      SnapshotEnvelopeError,
    )
  })
})

describe("snapshot-envelope binary v3 format", () => {
  it("round-trips a user payload, the principal and the constructor parameters", () => {
    const userBytes = new Uint8Array([1, 2, 3, 4, 5])
    const env = encodeBinaryEnvelope(header(oidcAlice), userBytes)
    expect(env.mimeType).toBe("application/octet-stream")
    expect(env.payload[0]).toBe(3) // version byte
    const decoded = decodeEnvelope(env)
    expect(decoded.kind).toBe("binary")
    if (decoded.kind !== "binary") throw new Error()
    expect(decoded.principal).toEqual(oidcAlice)
    expect(decoded.constructorParameters).toEqual(constructorParameters)
    expect(Array.from(decoded.userPayload)).toEqual([1, 2, 3, 4, 5])
  })

  it("uses big-endian length encoding for the JSON header", () => {
    const env = encodeBinaryEnvelope(header(), new Uint8Array(0))
    const view = new DataView(env.payload.buffer, env.payload.byteOffset, env.payload.byteLength)
    const headerLen = view.getUint32(1, false)
    expect(headerLen).toBe(env.payload.length - 5)
    const json = JSON.parse(new TextDecoder().decode(env.payload.slice(5)))
    expect(json.principal).toEqual(anonymous)
  })

  it.each([1, 2])("rejects the removed v%i binary envelope", (version) => {
    const payload = new Uint8Array([version, 0, 0, 0, 0])
    expect(() => decodeEnvelope({ payload, mimeType: "application/octet-stream" })).toThrow(
      SnapshotEnvelopeError,
    )
  })

  it("rejects truncated v3 headers", () => {
    const payload = new Uint8Array([3, 0, 0])
    expect(() => decodeEnvelope({ payload, mimeType: "application/octet-stream" })).toThrow(
      SnapshotEnvelopeError,
    )
  })

  it("rejects v3 payloads where the header length exceeds the buffer", () => {
    const payload = new Uint8Array(5)
    payload[0] = 3
    new DataView(payload.buffer).setUint32(1, 0xffff_ffff, false)
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

describe("snapshot-envelope constructor parameters", () => {
  it("round-trips bigints and bytes through JSON", () => {
    const json = JSON.parse(JSON.stringify(encodeConstructorParameters(constructorParameters)))
    expect(decodeConstructorParameters(json)).toEqual(constructorParameters)
  })

  it.each(["secret-value", "quota-token-handle", "permission-card-handle", "stream-value"])(
    "refuses to keep a %s node",
    (tag) => {
      const tree = { root: 0, valueNodes: [{ tag, val: {} }] } as never
      expect(() => encodeConstructorParameters(tree)).toThrow(SnapshotEnvelopeError)
      expect(() => decodeConstructorParameters(tree)).toThrow(SnapshotEnvelopeError)
    },
  )

  it.each([
    ["a non-tree", 7],
    ["a tree without nodes", { root: 0 }],
    [
      "a non-decimal integer",
      { root: 0, valueNodes: [{ tag: "u64-value", val: { $bigint: "01" } }] },
    ],
    ["an out-of-range byte", { root: 0, valueNodes: [{ tag: "x", val: { $bytes: [256] } }] }],
  ])("rejects %s", (_, value) => {
    expect(() => decodeConstructorParameters(value)).toThrow(SnapshotEnvelopeError)
  })
})

describe("snapshot-envelope unsupported formats", () => {
  it("rejects malformed multipart/mixed envelopes (no body)", () => {
    expect(() =>
      decodeEnvelope({ payload: new Uint8Array(0), mimeType: "multipart/mixed; boundary=abc" }),
    ).toThrow(SnapshotEnvelopeError)
  })

  it("rejects multipart/mixed without a boundary parameter", () => {
    expect(() =>
      decodeEnvelope({ payload: new Uint8Array(0), mimeType: "multipart/mixed" }),
    ).toThrow(UnsupportedSnapshotFormatError)
  })

  it("rejects unknown mime types", () => {
    expect(() => decodeEnvelope({ payload: new Uint8Array(0), mimeType: "text/plain" })).toThrow(
      UnsupportedSnapshotFormatError,
    )
  })
})
