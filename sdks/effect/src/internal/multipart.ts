/**
 * Minimal RFC-2046ish `multipart/mixed` encoder/decoder used by the
 * SQLite-aware snapshot envelope. Wire-compatible with the official
 * `golem-ts-sdk` multipart algorithm so components built with either
 * SDK can be cross-loaded.
 *
 * Wire format per part:
 *
 * ```
 * --<boundary>\r\n
 * Content-Type: <contentType>\r\n
 * Content-Disposition: attachment; name="<name>"\r\n
 * \r\n
 * <body bytes>
 * \r\n
 * ```
 *
 * Followed by `--<boundary>--\r\n` at the end. Header order is fixed
 * (`Content-Type` before `Content-Disposition`). The boundary is a
 * 32-character lowercase hex string (UUIDv4 with dashes stripped) and
 * is regenerated until it doesn't appear as a sequence
 * `\r\n--<boundary>` in any part body.
 *
 * Decoder accepts both `\r\n` and bare `\n` line endings (in headers
 * and after delimiter), rejects duplicate part names, and tolerates a
 * leading CRLF/LF before the first boundary.
 *
 * @since 1.5.0
 */

const CRLF = "\r\n"
const textEncoder = new TextEncoder()
const textDecoder = new TextDecoder()

/**
 * Tagged failure raised by {@link decodeMultipart} when the input is
 * malformed (no leading boundary, missing header terminator, missing or
 * duplicate part name, etc.). The owner of the multipart codec catches
 * this at its boundary and re-wraps it; users never see it directly.
 *
 * @internal
 * @since 1.5.0
 * @category errors
 */
export class MultipartCodecError extends Error {
  readonly _tag = "MultipartCodecError"
  constructor(readonly reason: string) {
    super(reason)
    this.name = "MultipartCodecError"
  }
}

/**
 * One named, content-typed part of a `multipart/mixed` body.
 *
 * @since 1.5.0
 * @category models
 */
export interface MultipartPart {
  readonly name: string
  readonly contentType: string
  readonly body: Uint8Array
}

const generateBoundary = (): string => {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID().replace(/-/g, "")
  }
  let hex = ""
  for (let i = 0; i < 32; i++) {
    hex += Math.floor(Math.random() * 16).toString(16)
  }
  return hex
}

const indexOf = (haystack: Uint8Array, needle: Uint8Array, from = 0): number => {
  if (needle.length === 0) return from
  if (needle.length > haystack.length - from) return -1
  outer: for (let i = from; i <= haystack.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) {
      if (haystack[i + j] !== needle[j]) continue outer
    }
    return i
  }
  return -1
}

const startsWith = (haystack: Uint8Array, needle: Uint8Array, from = 0): boolean => {
  if (needle.length > haystack.length - from) return false
  for (let i = 0; i < needle.length; i++) {
    if (haystack[from + i] !== needle[i]) return false
  }
  return true
}

const concat = (chunks: ReadonlyArray<Uint8Array>): Uint8Array => {
  let total = 0
  for (const c of chunks) total += c.length
  const out = new Uint8Array(total)
  let offset = 0
  for (const c of chunks) {
    out.set(c, offset)
    offset += c.length
  }
  return out
}

const containsBoundary = (parts: ReadonlyArray<MultipartPart>, boundary: string): boolean => {
  const marker = textEncoder.encode(`${CRLF}--${boundary}`)
  for (const part of parts) {
    if (indexOf(part.body, marker) !== -1) return true
  }
  return false
}

/**
 * Encode an array of parts into a multipart/mixed body. Returns the
 * encoded bytes alongside the chosen boundary (so the caller can
 * embed it into the Content-Type header).
 *
 * @since 1.5.0
 * @category operations
 */
export const encodeMultipart = (
  parts: ReadonlyArray<MultipartPart>,
): { readonly data: Uint8Array; readonly boundary: string } => {
  let boundary = generateBoundary()
  while (containsBoundary(parts, boundary)) {
    boundary = generateBoundary()
  }
  const chunks: Array<Uint8Array> = []
  for (const part of parts) {
    const header =
      `--${boundary}${CRLF}` +
      `Content-Type: ${part.contentType}${CRLF}` +
      `Content-Disposition: attachment; name="${part.name}"${CRLF}` +
      CRLF
    chunks.push(textEncoder.encode(header))
    chunks.push(part.body)
    chunks.push(textEncoder.encode(CRLF))
  }
  chunks.push(textEncoder.encode(`--${boundary}--${CRLF}`))
  return { data: concat(chunks), boundary }
}

/**
 * Split `data[from..]` on every occurrence of `delimiter`, returning
 * the slices between consecutive matches (excluding the delimiter
 * itself).
 */
const splitOnDelimiter = (
  data: Uint8Array,
  delimiter: Uint8Array,
  from: number,
): Array<Uint8Array> => {
  const sections: Array<Uint8Array> = []
  let cursor = from
  while (cursor <= data.length) {
    const next = indexOf(data, delimiter, cursor)
    if (next === -1) {
      sections.push(data.slice(cursor))
      break
    }
    sections.push(data.slice(cursor, next))
    cursor = next + delimiter.length
  }
  return sections
}

/** Find the first `\r\n\r\n` at or after `start`; returns -1 on miss. */
const findDoubleCrlf = (data: Uint8Array, start: number): number => {
  for (let i = start; i + 3 < data.length; i++) {
    if (data[i] === 0x0d && data[i + 1] === 0x0a && data[i + 2] === 0x0d && data[i + 3] === 0x0a) {
      return i
    }
  }
  return -1
}

/** Find the first `\n\n` at or after `start`; returns -1 on miss. */
const findDoubleLf = (data: Uint8Array, start: number): number => {
  for (let i = start; i + 1 < data.length; i++) {
    if (data[i] === 0x0a && data[i + 1] === 0x0a) return i
  }
  return -1
}

/**
 * Decode a multipart/mixed body. Throws on malformed input or
 * duplicate part names. Both `\r\n` and bare `\n` line endings are
 * tolerated (we always split on `\n--<boundary>` and strip a trailing
 * `\r` from each section).
 *
 * @since 1.5.0
 * @category operations
 */
export const decodeMultipart = (data: Uint8Array, boundary: string): Array<MultipartPart> => {
  // Split on `\n--<boundary>` so that both `\r\n` and bare `\n` line
  // endings work; we then strip a trailing `\r` from each section
  // body (and from each header line) if present.
  const delimiter = textEncoder.encode(`\n--${boundary}`)
  const firstDelimiter = textEncoder.encode(`--${boundary}`)

  // Strip leading CRLF or LF.
  let start = 0
  if (data.length >= 2 && data[0] === 0x0d && data[1] === 0x0a) start = 2
  else if (data.length >= 1 && data[0] === 0x0a) start = 1

  if (!startsWith(data, firstDelimiter, start)) {
    throw new MultipartCodecError("body does not start with boundary")
  }

  const rawSections = splitOnDelimiter(data, delimiter, start + firstDelimiter.length)
  const parts: Array<MultipartPart> = []
  const seen = new Set<string>()

  for (const section of rawSections) {
    // Skip closing delimiter ("--" suffix).
    if (startsWith(section, textEncoder.encode("--"), 0)) continue

    // Skip CRLF or LF after the delimiter.
    let s = 0
    if (section.length >= 2 && section[0] === 0x0d && section[1] === 0x0a) s = 2
    else if (section.length >= 1 && section[0] === 0x0a) s = 1

    const headerEndCrlf = findDoubleCrlf(section, s)
    const headerEndLf = findDoubleLf(section, s)
    let headerEnd: number
    let bodyStart: number
    if (headerEndCrlf !== -1 && (headerEndLf === -1 || headerEndCrlf <= headerEndLf)) {
      headerEnd = headerEndCrlf
      bodyStart = headerEndCrlf + 4
    } else if (headerEndLf !== -1) {
      headerEnd = headerEndLf
      bodyStart = headerEndLf + 2
    } else {
      throw new MultipartCodecError("could not find end of headers in part")
    }

    const headerText = textDecoder.decode(section.slice(s, headerEnd))
    const headerLines = headerText.split(/\r?\n/)

    let contentType = ""
    let name = ""
    for (const line of headerLines) {
      const colon = line.indexOf(":")
      if (colon === -1) continue
      const key = line.slice(0, colon).trim().toLowerCase()
      const value = line.slice(colon + 1).trim()
      if (key === "content-type") contentType = value
      else if (key === "content-disposition") {
        const m = value.match(/name="([^"]+)"/)
        if (m) name = m[1]!
      }
    }

    if (!name) throw new MultipartCodecError("part missing name in Content-Disposition")
    if (seen.has(name)) throw new MultipartCodecError(`duplicate part name: ${name}`)
    seen.add(name)

    // Strip a single trailing `\r` from the body (the splitter already
    // consumed the `\n` of the CRLF that preceded the next delimiter).
    let bodyEnd = section.length
    if (bodyEnd > bodyStart && section[bodyEnd - 1] === 0x0d) {
      bodyEnd -= 1
    }

    parts.push({ name, contentType, body: section.slice(bodyStart, bodyEnd) })
  }

  return parts
}

/**
 * Extract the `boundary` parameter from a `multipart/mixed; boundary=…`
 * mime type. Returns `null` if the parameter is missing/malformed.
 *
 * @since 1.5.0
 * @category utils
 */
export const extractBoundary = (mimeType: string): string | null => {
  const m = mimeType.match(/boundary=([^\s;"]+|"[^"]+")/i)
  if (!m) return null
  const raw = m[1]!
  if (raw.startsWith('"') && raw.endsWith('"')) return raw.slice(1, -1)
  return raw
}
