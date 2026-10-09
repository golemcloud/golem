import { Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as ApiHost from "golem:api/host@1.5.0"
import * as CoreTypes from "golem:core/types@2.0.0"
import type { SnapshotPart } from "../Snapshot.js"
import {
  decodeMultipart,
  encodeMultipart,
  extractBoundary,
  MultipartCodecError,
} from "./multipart.js"
import { strictTextDecoder } from "./textDecoder.js"

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
const USER_PART_PREFIX = "part:"

/** Validate logical user names before prefixing or inserting into maps. */
const validatePartName = (name: string): void => {
  if (!/^[A-Za-z0-9_][A-Za-z0-9_.-]*$/.test(name)) {
    throw new SnapshotEnvelopeError(`invalid snapshot part name '${name}'`)
  }
}

/** Bare ASCII MIME type/subtype; parameters have no user-part semantics. */
export const normalizeSnapshotContentType = (contentType: string): string => {
  if (!/^[A-Za-z0-9!#$%&'*+.^_`|~-]+\/[A-Za-z0-9!#$%&'*+.^_`|~-]+$/.test(contentType)) {
    throw new SnapshotEnvelopeError(`invalid snapshot part Content-Type '${contentType}'`)
  }
  return contentType.toLowerCase()
}

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
  fileDatabases: Schema.optional(Schema.Record(Schema.String, Schema.String)),
})

const EnvelopeFromString = Schema.fromJsonString(Envelope)
const PrincipalFromString = Schema.fromJsonString(SerializedPrincipal)

const encodeEnvelope = Schema.encodeUnknownSync(EnvelopeFromString)
const decodeEnvelopeFromString = Schema.decodeUnknownSync(EnvelopeFromString)
const encodePrincipal = Schema.encodeUnknownSync(PrincipalFromString)
const decodePrincipalFromString = Schema.decodeUnknownSync(PrincipalFromString)

const encoder = new TextEncoder()
const strictDecoder = strictTextDecoder()

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
 * carries `{ version: 1, principal, state, fileDatabases? }` as JSON; each entry in
 * `databases` becomes a `db:<name>` part with `application/x-sqlite3`
 * content-type. File-backed database contents belong to filesystem snapshots;
 * only their logical names and locations belong in `fileDatabases`.
 */
export const encodeMultipartJsonEnvelope = (
  principal: AgentCommon.Principal,
  state: unknown,
  databases: ReadonlyArray<{ readonly name: string; readonly bytes: Uint8Array }>,
  userParts: ReadonlyMap<string, SnapshotPart> = new Map(),
  fileDatabases: Readonly<Record<string, string>> | undefined = databases.length > 0
    ? {}
    : undefined,
): ApiHost.Snapshot => {
  if (state === undefined)
    throw new SnapshotEnvelopeError(
      "multipart state must be JSON; use null for binary-only snapshots",
    )
  const stateJson = encodeEnvelope({
    version: 1,
    principal: serializePrincipal(principal),
    state,
    ...(fileDatabases === undefined ? {} : { fileDatabases }),
  })
  const stateBody = encoder.encode(stateJson)
  const parts = [
    { name: STATE_PART_NAME, contentType: JSON_MIME, body: stateBody },
    ...databases.map((db) => ({
      name: `${DB_PART_PREFIX}${db.name}`,
      contentType: SQLITE_PART_MIME,
      body: db.bytes,
    })),
    ...Array.from(userParts, ([name, part]) => {
      validatePartName(name)
      if (!(part.bytes instanceof Uint8Array))
        throw new SnapshotEnvelopeError(`part '${name}' bytes must be Uint8Array`)
      return {
        name: `${USER_PART_PREFIX}${name}`,
        contentType: normalizeSnapshotContentType(part.contentType),
        body: part.bytes,
      }
    }),
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
 * stripped); each `databases` entry carries an in-memory/temp SQLite image.
 * File-backed databases carry locations only, never contents.
 */
export interface DecodedMultipartEnvelope {
  readonly kind: "multipart"
  readonly principal: AgentCommon.Principal
  readonly state: unknown
  readonly databases: ReadonlyArray<{ readonly name: string; readonly bytes: Uint8Array }>
  readonly fileDatabases: Readonly<Record<string, string>> | undefined
  readonly parts: ReadonlyMap<string, SnapshotPart>
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
  if (mime.split(";", 1)[0]?.trim().toLowerCase() === MULTIPART_MIME_PREFIX) {
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

const parseEnvelopeJson = (text: string, ctx: string, strict = false) => {
  try {
    if (strict) {
      // JSON.parse validates syntax but discards duplicate keys. Inspect the
      // valid token stream before schema decoding so metadata cannot be replaced.
      const value = JSON.parse(text)
      if (value === null || typeof value !== "object" || !Object.hasOwn(value, "state")) {
        throw new Error("multipart envelope missing state")
      }
      const tokens = text.match(/"(?:[^"\\]|\\.)*"|[{}[\],:]|[^{}[\],:\s]+/g)!
      const objects: Array<Set<string> | undefined> = []
      for (let i = 0; i < tokens.length; i++) {
        const token = tokens[i]!
        if (token === "{") objects.push(new Set())
        else if (token === "[") objects.push(undefined)
        else if (token === "}" || token === "]") objects.pop()
        else if (token.startsWith('"') && tokens[i + 1] === ":") {
          const key: string = JSON.parse(token)
          const keys = objects[objects.length - 1]!
          if (keys.has(key)) throw new Error(`duplicate JSON key '${key}'`)
          keys.add(key)
          if (objects.length === 1 && key === "version" && tokens[i + 2] !== "1") {
            throw new Error("multipart envelope version must be integer 1")
          }
        }
      }
    }
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
    if (err instanceof MultipartCodecError) {
      throw new SnapshotEnvelopeError(`multipart envelope: ${err.reason}`)
    }
    throw err
  }

  const statePart = parts.find((p) => p.name === STATE_PART_NAME)
  if (!statePart) {
    throw new SnapshotEnvelopeError(`multipart envelope: missing 'state' part`)
  }
  if (statePart.contentType !== JSON_MIME) {
    throw new SnapshotEnvelopeError(
      `multipart envelope: 'state' part has Content-Type '${statePart.contentType}' (expected '${JSON_MIME}')`,
    )
  }
  const stateText = decodeUtf8(statePart.body, "multipart envelope: 'state' part")
  // Enforce strict UTF-8 even when the guest runtime lacks fatal TextDecoder.
  const reencoded = encoder.encode(stateText)
  if (
    reencoded.length !== statePart.body.length ||
    reencoded.some((b, i) => b !== statePart.body[i])
  ) {
    throw new SnapshotEnvelopeError("multipart envelope state is not canonical UTF-8")
  }
  const env = parseEnvelopeJson(stateText, "multipart envelope: 'state' part", true)
  const principal = deserializePrincipal(env.principal)

  const databases: Array<{ name: string; bytes: Uint8Array }> = []
  const userParts = new Map<string, SnapshotPart>()
  for (const part of parts) {
    if (part.name === STATE_PART_NAME) continue
    if (part.name.startsWith(USER_PART_PREFIX)) {
      const name = part.name.slice(USER_PART_PREFIX.length)
      validatePartName(name)
      userParts.set(name, {
        bytes: part.body,
        contentType: normalizeSnapshotContentType(part.contentType),
      })
      continue
    }
    if (!part.name.startsWith(DB_PART_PREFIX)) {
      throw new SnapshotEnvelopeError(
        `multipart envelope: unrecognised part name '${part.name}' (expected 'state', 'part:<name>' or 'db:<name>')`,
      )
    }
    if (part.contentType !== SQLITE_PART_MIME) {
      throw new SnapshotEnvelopeError(
        `multipart envelope: '${part.name}' part has Content-Type '${part.contentType}' (expected '${SQLITE_PART_MIME}')`,
      )
    }
    const dbName = part.name.slice(DB_PART_PREFIX.length)
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(dbName))
      throw new SnapshotEnvelopeError(`invalid database name '${dbName}'`)
    databases.push({ name: dbName, bytes: part.body })
  }

  if (env.fileDatabases !== undefined) {
    for (const [name, location] of Object.entries(env.fileDatabases)) {
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name))
        throw new SnapshotEnvelopeError(`invalid database name '${name}'`)
      if (location.length === 0 || location === ":memory:" || location.includes("\0"))
        throw new SnapshotEnvelopeError(`invalid file database location for '${name}'`)
      if (databases.some((part) => part.name === name))
        throw new SnapshotEnvelopeError(`database '${name}' has both an image and a file location`)
    }
  }
  return {
    kind: "multipart",
    principal,
    state: env.state,
    databases,
    fileDatabases: env.fileDatabases,
    parts: userParts,
  }
}

const decodeBinaryEnvelope = (
  payload: Uint8Array,
  _fallbackPrincipal: AgentCommon.Principal,
): DecodedBinaryEnvelope => {
  if (payload.length < 1) {
    throw new SnapshotEnvelopeError(`binary envelope: payload is empty`)
  }
  const version = payload[0]!
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
