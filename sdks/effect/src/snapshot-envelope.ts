import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as ApiHost from "golem:api/host@1.5.0"
import * as CoreTypes from "golem:core/types@1.5.0"
import { decodeMultipart, encodeMultipart, extractBoundary } from "./multipart.js"

/**
 * Snapshot envelope formats used by Golem agents.
 *
 * `effect-golem` is wire-compatible with the official `golem-ts-sdk`
 * snapshotting envelopes so that components built with either SDK can
 * be cross-loaded.
 *
 * Two formats are produced by `effect-golem`:
 *
 * - **JSON envelope** (`mimeType: "application/json"`) — used by the
 *   schema-driven `Snapshot.define(...)` variant. The payload is the
 *   UTF-8 encoding of `{ version: 1, principal, state }`, where `state`
 *   is the user-state JSON encoded by the user's Schema.
 * - **Binary envelope** (`mimeType: "application/octet-stream"`) — used
 *   by the user-managed `Snapshot.custom(...)` variant. The payload is
 *   `[u8 version=2][u32 BE princLen][princJson][userBytes]`.
 *
 * The decoder additionally accepts the legacy binary `version = 1`
 * format (no embedded principal — caller supplies a fallback) for
 * forward compatibility with snapshots produced by older `golem-ts-sdk`
 * components.
 *
 * `multipart/mixed` envelopes (used by the official SDK when SQLite
 * databases are present) are intentionally rejected here with a clear
 * error: `effect-golem` does not yet have a SQLite story.
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

/**
 * The shape of a {@link AgentCommon.Principal} as it appears *inside the
 * envelope* — UUIDs are serialized as strings (not `{ highBits, lowBits }`)
 * so the payload survives `JSON.stringify`. This format is bit-for-bit
 * compatible with the official `golem-ts-sdk` envelope.
 */
export type SerializedPrincipal =
  | { readonly tag: "anonymous" }
  | {
      readonly tag: "agent"
      readonly val: { readonly componentId: string; readonly agentId: string }
    }
  | { readonly tag: "golem-user"; readonly val: { readonly accountId: string } }
  | {
      readonly tag: "oidc"
      readonly val: {
        readonly sub: string
        readonly issuer: string
        readonly email: string | null
        readonly name: string | null
        readonly emailVerified: boolean | null
        readonly givenName: string | null
        readonly familyName: string | null
        readonly picture: string | null
        readonly preferredUsername: string | null
        readonly claims: string
      }
    }

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
  const envelope = { version: 1, principal: serializePrincipal(principal), state }
  const json = JSON.stringify(envelope)
  return {
    payload: new TextEncoder().encode(json),
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
  const principalJson = JSON.stringify(serializePrincipal(principal))
  const principalBytes = new TextEncoder().encode(principalJson)
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
  const envelope = { version: 1, principal: serializePrincipal(principal), state }
  const stateBody = new TextEncoder().encode(JSON.stringify(envelope))
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

const decodeJsonEnvelope = (payload: Uint8Array): DecodedJsonEnvelope => {
  let text: string
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(payload)
  } catch (err) {
    throw new SnapshotEnvelopeError(`json envelope: payload is not valid UTF-8: ${String(err)}`)
  }
  let parsed: unknown
  try {
    parsed = JSON.parse(text)
  } catch (err) {
    throw new SnapshotEnvelopeError(`json envelope: payload is not valid JSON: ${String(err)}`)
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new SnapshotEnvelopeError(`json envelope: expected an object, got ${typeof parsed}`)
  }
  const obj = parsed as Record<string, unknown>
  if (obj.version !== 1) {
    throw new SnapshotEnvelopeError(
      `json envelope: unsupported version ${String(obj.version)} (expected 1)`,
    )
  }
  if (!("principal" in obj)) {
    throw new SnapshotEnvelopeError(`json envelope: missing 'principal' field`)
  }
  if (!("state" in obj)) {
    throw new SnapshotEnvelopeError(`json envelope: missing 'state' field`)
  }
  return {
    kind: "json",
    principal: deserializePrincipal(obj.principal as SerializedPrincipal),
    state: obj.state,
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
  let stateText: string
  try {
    stateText = new TextDecoder("utf-8", { fatal: true }).decode(statePart.body)
  } catch (err) {
    throw new SnapshotEnvelopeError(
      `multipart envelope: 'state' part is not valid UTF-8: ${String(err)}`,
    )
  }
  let parsed: unknown
  try {
    parsed = JSON.parse(stateText)
  } catch (err) {
    throw new SnapshotEnvelopeError(
      `multipart envelope: 'state' part is not valid JSON: ${String(err)}`,
    )
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new SnapshotEnvelopeError(
      `multipart envelope: 'state' part expected an object, got ${typeof parsed}`,
    )
  }
  const obj = parsed as Record<string, unknown>
  if (obj.version !== 1) {
    throw new SnapshotEnvelopeError(
      `multipart envelope: unsupported state version ${String(obj.version)} (expected 1)`,
    )
  }
  if (!("principal" in obj)) {
    throw new SnapshotEnvelopeError(`multipart envelope: 'state' part missing 'principal' field`)
  }
  if (!("state" in obj)) {
    throw new SnapshotEnvelopeError(`multipart envelope: 'state' part missing 'state' field`)
  }
  const principal = deserializePrincipal(obj.principal as SerializedPrincipal)
  const state = obj.state

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

  return { kind: "multipart", principal, state, databases }
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
    let principalJson: string
    try {
      principalJson = new TextDecoder("utf-8", { fatal: true }).decode(principalBytes)
    } catch (err) {
      throw new SnapshotEnvelopeError(
        `binary envelope (v2): principal segment is not valid UTF-8: ${String(err)}`,
      )
    }
    let serialized: SerializedPrincipal
    try {
      serialized = JSON.parse(principalJson) as SerializedPrincipal
    } catch (err) {
      throw new SnapshotEnvelopeError(
        `binary envelope (v2): principal segment is not valid JSON: ${String(err)}`,
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
