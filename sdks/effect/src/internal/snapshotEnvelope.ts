import { Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as ApiHost from "golem:api/host@1.5.0"
import * as CoreTypes from "golem:core/types@1.5.0"
import { decodeMultipart, encodeMultipart, extractBoundary } from "./multipart.js"

/**
 * @internal
 * @since 1.5.0
 */

/** A typed failure for envelope encode/decode operations. */
export class SnapshotEnvelopeError {
  readonly _tag = "SnapshotEnvelopeError"
  constructor(readonly reason: string) {}
}

/** A typed failure raised when a snapshot mime type is not supported. */
export class UnsupportedSnapshotFormatError {
  readonly _tag = "UnsupportedSnapshotFormatError"
  constructor(readonly mimeType: string) {}
}

const JSON_MIME = "application/json"
const BINARY_MIME = "application/octet-stream"
const MULTIPART_MIME_PREFIX = "multipart/mixed"
const SQLITE_PART_MIME = "application/x-sqlite3"
const STATE_PART_NAME = "state"
const DB_PART_PREFIX = "db:"

// ---------------------------------------------------------------------------
// Schemas — the JSON-safe wire shapes
// ---------------------------------------------------------------------------

/**
 * Each OIDC string-typed sub-claim is encoded as `T | null` on the
 * wire (we explicitly emit `null` for absent fields), and tolerated
 * as missing on decode (so payloads produced by other SDKs that
 * simply omit absent claims still parse).
 */
const NullishString = Schema.optional(Schema.NullOr(Schema.String))
const NullishBoolean = Schema.optional(Schema.NullOr(Schema.Boolean))

/**
 * Schema for the JSON-safe `Principal` shape (UUIDs as strings).
 * Mirrors the runtime {@link AgentCommon.Principal} ADT but with all
 * `bigint` UUID payloads stringified so the value survives
 * `JSON.stringify` / `JSON.parse`.
 */
export const SerializedPrincipal = Schema.Union([
  Schema.Struct({ tag: Schema.Literal("anonymous") }),
  Schema.Struct({
    tag: Schema.Literal("agent"),
    val: Schema.Struct({
      componentId: Schema.String,
      agentId: Schema.String,
    }),
  }),
  Schema.Struct({
    tag: Schema.Literal("golem-user"),
    val: Schema.Struct({ accountId: Schema.String }),
  }),
  Schema.Struct({
    tag: Schema.Literal("oidc"),
    val: Schema.Struct({
      sub: Schema.String,
      issuer: Schema.String,
      email: NullishString,
      name: NullishString,
      emailVerified: NullishBoolean,
      givenName: NullishString,
      familyName: NullishString,
      picture: NullishString,
      preferredUsername: NullishString,
      claims: Schema.String,
    }),
  }),
])

/** Type alias for the JSON-safe principal value. */
export type SerializedPrincipal = typeof SerializedPrincipal.Type

/**
 * Schema for the full envelope: `{ version: 1, principal, state }`.
 * The `state` slot is intentionally `Schema.Unknown` — the user's own
 * Schema has already encoded it to a JSON value at the moment we
 * build the envelope, and re-validation happens against the user's
 * Schema after decode.
 */
const Envelope = Schema.Struct({
  version: Schema.Literal(1),
  principal: SerializedPrincipal,
  state: Schema.Unknown,
})

const EnvelopeFromString = Schema.fromJsonString(Envelope)
const PrincipalFromString = Schema.fromJsonString(SerializedPrincipal)

const encodeEnvelope = Schema.encodeUnknownSync(EnvelopeFromString)
const decodeEnvelopeFromString = Schema.decodeUnknownSync(EnvelopeFromString)
const encodePrincipal = Schema.encodeUnknownSync(PrincipalFromString)
const decodePrincipalFromString = Schema.decodeUnknownSync(PrincipalFromString)

const encoder = new TextEncoder()
const strictDecoder = new TextDecoder("utf-8", { fatal: true })

const decodeUtf8 = (bytes: Uint8Array, ctx: string): string => {
  try {
    return strictDecoder.decode(bytes)
  } catch (err) {
    throw new SnapshotEnvelopeError(`${ctx}: payload is not valid UTF-8: ${String(err)}`)
  }
}

// ---------------------------------------------------------------------------
// Principal: runtime ↔ wire conversion
// ---------------------------------------------------------------------------

/**
 * Convert a runtime `Principal` (with bigint UUIDs) into a JSON-safe
 * envelope shape (with stringified UUIDs).
 */
export const serializePrincipal = (p: AgentCommon.Principal): SerializedPrincipal => {
  switch (p.tag) {
    case "anonymous":
      return { tag: "anonymous" }
    case "agent":
      return {
        tag: "agent",
        val: {
          componentId: CoreTypes.uuidToString(p.val.agentId.componentId.uuid),
          agentId: p.val.agentId.agentId,
        },
      }
    case "golem-user":
      return {
        tag: "golem-user",
        val: { accountId: CoreTypes.uuidToString(p.val.accountId.uuid) },
      }
    case "oidc":
      return {
        tag: "oidc",
        val: {
          sub: p.val.sub,
          issuer: p.val.issuer,
          email: p.val.email ?? null,
          name: p.val.name ?? null,
          emailVerified: p.val.emailVerified ?? null,
          givenName: p.val.givenName ?? null,
          familyName: p.val.familyName ?? null,
          picture: p.val.picture ?? null,
          preferredUsername: p.val.preferredUsername ?? null,
          claims: p.val.claims,
        },
      }
  }
}

/**
 * Inverse of {@link serializePrincipal}: take the JSON-safe envelope
 * shape and rebuild the runtime `Principal` (parsing UUID strings back
 * into `{ highBits, lowBits }`).
 */
export const deserializePrincipal = (p: SerializedPrincipal): AgentCommon.Principal => {
  switch (p.tag) {
    case "anonymous":
      return { tag: "anonymous" }
    case "agent":
      return {
        tag: "agent",
        val: {
          agentId: {
            componentId: { uuid: CoreTypes.parseUuid(p.val.componentId) },
            agentId: p.val.agentId,
          },
        },
      }
    case "golem-user":
      return {
        tag: "golem-user",
        val: { accountId: { uuid: CoreTypes.parseUuid(p.val.accountId) } },
      }
    case "oidc":
      return {
        tag: "oidc",
        val: {
          sub: p.val.sub,
          issuer: p.val.issuer,
          email: p.val.email ?? undefined,
          name: p.val.name ?? undefined,
          emailVerified: p.val.emailVerified ?? undefined,
          givenName: p.val.givenName ?? undefined,
          familyName: p.val.familyName ?? undefined,
          picture: p.val.picture ?? undefined,
          preferredUsername: p.val.preferredUsername ?? undefined,
          claims: p.val.claims,
        },
      }
  }
}

// ---------------------------------------------------------------------------
// Encode
// ---------------------------------------------------------------------------

/**
 * Encode an `application/json` envelope. `state` is an arbitrary JSON
 * value (already produced by the user's Schema-driven encode step).
 * Accepts a runtime `Principal` (with bigint UUIDs) and serializes it
 * via {@link serializePrincipal} before writing.
 */
export const encodeJsonEnvelope = (
  principal: AgentCommon.Principal,
  state: unknown,
): ApiHost.Snapshot => {
  const json = encodeEnvelope({
    version: 1,
    principal: serializePrincipal(principal),
    state,
  })
  return {
    payload: encoder.encode(json),
    mimeType: JSON_MIME,
  }
}

/**
 * Encode an `application/octet-stream` envelope (binary v2). The user's
 * raw `payload` bytes are appended verbatim after the principal header.
 * Accepts a runtime `Principal` (with bigint UUIDs) and serializes it
 * via {@link serializePrincipal} before writing.
 */
export const encodeBinaryEnvelope = (
  principal: AgentCommon.Principal,
  userPayload: Uint8Array,
): ApiHost.Snapshot => {
  const principalJson = encodePrincipal(serializePrincipal(principal))
  const principalBytes = encoder.encode(principalJson)
  const total = 1 + 4 + principalBytes.length + userPayload.length
  const out = new Uint8Array(total)
  const view = new DataView(out.buffer)
  view.setUint8(0, 2)
  view.setUint32(1, principalBytes.length, false) // big-endian
  out.set(principalBytes, 5)
  out.set(userPayload, 5 + principalBytes.length)
  return { payload: out, mimeType: BINARY_MIME }
}

/**
 * Encode a SQLite-aware `multipart/mixed` envelope. The `state` part
 * carries `{ version: 1, principal, state }` as JSON; each entry in
 * `databases` becomes a `db:<name>` part with `application/x-sqlite3`
 * content-type. Bit-compatible with `golem-ts-sdk`.
 */
export const encodeMultipartJsonEnvelope = (
  principal: AgentCommon.Principal,
  state: unknown,
  databases: ReadonlyArray<{ readonly name: string; readonly bytes: Uint8Array }>,
): ApiHost.Snapshot => {
  const stateJson = encodeEnvelope({
    version: 1,
    principal: serializePrincipal(principal),
    state,
  })
  const stateBody = encoder.encode(stateJson)
  const parts = [
    { name: STATE_PART_NAME, contentType: JSON_MIME, body: stateBody },
    ...databases.map((db) => ({
      name: `${DB_PART_PREFIX}${db.name}`,
      contentType: SQLITE_PART_MIME,
      body: db.bytes,
    })),
  ]
  const { data, boundary } = encodeMultipart(parts)
  return { payload: data, mimeType: `${MULTIPART_MIME_PREFIX}; boundary=${boundary}` }
}

// ---------------------------------------------------------------------------
// Decode
// ---------------------------------------------------------------------------

/**
 * Decoded JSON envelope. The `principal` is the recovered runtime
 * principal (UUIDs back to bigints); `state` is whatever JSON shape the
 * user's `Schema.encodeUnknown` produced when the snapshot was saved.
 */
export interface DecodedJsonEnvelope {
  readonly kind: "json"
  readonly principal: AgentCommon.Principal
  readonly state: unknown
}

/**
 * Decoded binary envelope. The `userPayload` is the raw byte slice the
 * user's `save` Effect originally returned.
 */
export interface DecodedBinaryEnvelope {
  readonly kind: "binary"
  readonly principal: AgentCommon.Principal
  readonly userPayload: Uint8Array
}

/**
 * Decoded multipart envelope produced by the SQLite-aware
 * {@link encodeMultipartJsonEnvelope} (or by the official
 * `golem-ts-sdk`). The `state` is the user-state JSON value (the
 * outer `{version, principal, state}` envelope has already been
 * stripped); each `databases` entry carries the raw SQLite file bytes
 * for one declared `db:<name>` part.
 */
export interface DecodedMultipartEnvelope {
  readonly kind: "multipart"
  readonly principal: AgentCommon.Principal
  readonly state: unknown
  readonly databases: ReadonlyArray<{ readonly name: string; readonly bytes: Uint8Array }>
}

export type DecodedEnvelope = DecodedJsonEnvelope | DecodedBinaryEnvelope | DecodedMultipartEnvelope

/**
 * Decode a {@link ApiHost.Snapshot}. `fallbackPrincipal` is used when
 * the snapshot is the legacy binary v1 format which carries no
 * principal in its payload.
 */
export const decodeEnvelope = (
  snapshot: ApiHost.Snapshot,
  fallbackPrincipal: AgentCommon.Principal,
): DecodedEnvelope => {
  const mime = snapshot.mimeType
  if (mime.startsWith(MULTIPART_MIME_PREFIX)) {
    return decodeMultipartEnvelope(snapshot.payload, mime)
  }
  if (mime === JSON_MIME) {
    return decodeJsonEnvelope(snapshot.payload)
  }
  if (mime === BINARY_MIME) {
    return decodeBinaryEnvelope(snapshot.payload, fallbackPrincipal)
  }
  throw new UnsupportedSnapshotFormatError(mime)
}

const parseEnvelopeJson = (text: string, ctx: string) => {
  try {
    return decodeEnvelopeFromString(text)
  } catch (err) {
    throw new SnapshotEnvelopeError(`${ctx}: ${String((err as Error).message ?? err)}`)
  }
}

const decodeJsonEnvelope = (payload: Uint8Array): DecodedJsonEnvelope => {
  const text = decodeUtf8(payload, "json envelope")
  const env = parseEnvelopeJson(text, "json envelope")
  return {
    kind: "json",
    principal: deserializePrincipal(env.principal),
    state: env.state,
  }
}

const decodeMultipartEnvelope = (payload: Uint8Array, mime: string): DecodedMultipartEnvelope => {
  const boundary = extractBoundary(mime)
  if (boundary === null) {
    throw new UnsupportedSnapshotFormatError(mime)
  }
  let parts: ReturnType<typeof decodeMultipart>
  try {
    parts = decodeMultipart(payload, boundary)
  } catch (err) {
    throw new SnapshotEnvelopeError(`multipart envelope: ${String((err as Error).message ?? err)}`)
  }

  const statePart = parts.find((p) => p.name === STATE_PART_NAME)
  if (!statePart) {
    throw new SnapshotEnvelopeError(`multipart envelope: missing 'state' part`)
  }
  const stateText = decodeUtf8(statePart.body, "multipart envelope: 'state' part")
  const env = parseEnvelopeJson(stateText, "multipart envelope: 'state' part")
  const principal = deserializePrincipal(env.principal)

  const databases: Array<{ name: string; bytes: Uint8Array }> = []
  for (const part of parts) {
    if (part.name === STATE_PART_NAME) continue
    if (!part.name.startsWith(DB_PART_PREFIX)) {
      throw new SnapshotEnvelopeError(
        `multipart envelope: unrecognised part name '${part.name}' (expected 'state' or 'db:<name>')`,
      )
    }
    const dbName = part.name.slice(DB_PART_PREFIX.length)
    databases.push({ name: dbName, bytes: part.body })
  }

  return { kind: "multipart", principal, state: env.state, databases }
}

const decodeBinaryEnvelope = (
  payload: Uint8Array,
  fallbackPrincipal: AgentCommon.Principal,
): DecodedBinaryEnvelope => {
  if (payload.length < 1) {
    throw new SnapshotEnvelopeError(`binary envelope: payload is empty`)
  }
  const version = payload[0]!
  if (version === 1) {
    // Legacy: byte 0 = 1, no embedded principal; the rest is the raw
    // user payload. Use the supplied fallback principal.
    return {
      kind: "binary",
      principal: fallbackPrincipal,
      userPayload: payload.slice(1),
    }
  }
  if (version === 2) {
    if (payload.length < 5) {
      throw new SnapshotEnvelopeError(`binary envelope (v2): truncated header`)
    }
    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength)
    const principalLen = view.getUint32(1, false)
    if (payload.length < 5 + principalLen) {
      throw new SnapshotEnvelopeError(
        `binary envelope (v2): declared principal length ${principalLen} exceeds payload size ${payload.length - 5}`,
      )
    }
    const principalBytes = payload.slice(5, 5 + principalLen)
    const principalText = decodeUtf8(principalBytes, "binary envelope (v2): principal segment")
    let serialized: SerializedPrincipal
    try {
      serialized = decodePrincipalFromString(principalText)
    } catch (err) {
      throw new SnapshotEnvelopeError(
        `binary envelope (v2): principal segment is not a valid principal: ${String((err as Error).message ?? err)}`,
      )
    }
    return {
      kind: "binary",
      principal: deserializePrincipal(serialized),
      userPayload: payload.slice(5 + principalLen),
    }
  }
  throw new SnapshotEnvelopeError(`binary envelope: unsupported version byte ${version}`)
}
