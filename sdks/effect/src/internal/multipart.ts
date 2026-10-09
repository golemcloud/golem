/** Byte-preserving multipart/mixed framing shared by snapshot modes. @internal @since 1.5.0 */
const encoder = new TextEncoder()
const decoder = new TextDecoder()
const CRLF = "\r\n"

/** Malformed multipart framing or headers. @since 1.5.0 @category errors */
export class MultipartCodecError extends Error {
  readonly _tag = "MultipartCodecError"
  constructor(readonly reason: string) {
    super(reason)
    this.name = "MultipartCodecError"
  }
}

/** A named opaque body. Namespace policy belongs to the envelope. @since 1.5.0 @category models */
export interface MultipartPart {
  readonly name: string
  readonly contentType: string
  readonly body: Uint8Array
}

const validBoundary = (boundary: string): boolean =>
  /^[A-Za-z0-9'()+_,./:=?-]{1,70}$/.test(boundary)

const matches = (data: Uint8Array, text: Uint8Array, offset: number): boolean => {
  if (offset < 0 || offset + text.length > data.length) return false
  for (let i = 0; i < text.length; i++) {
    if (data[offset + i] !== text[i]) return false
  }
  return true
}

const concat = (chunks: ReadonlyArray<Uint8Array>): Uint8Array => {
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0))
  let offset = 0
  for (const chunk of chunks) {
    out.set(chunk, offset)
    offset += chunk.length
  }
  return out
}

// Returns the suffix position only for complete delimiter lines.
const delimiterAt = (
  data: Uint8Array,
  marker: Uint8Array,
  newline: Uint8Array,
  offset: number,
): { readonly end: number; readonly closing: boolean } | undefined => {
  if (!matches(data, marker, offset)) return undefined
  let end = offset + marker.length
  const closing = data[end] === 45 && data[end + 1] === 45
  if (closing) end += 2
  if (matches(data, newline, end)) return { end: end + newline.length, closing }
  if (closing && end === data.length) return { end, closing }
  return undefined
}

const generateBoundary = (): string => {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID().replace(/-/g, "")
  }
  let hex = ""
  for (let i = 0; i < 32; i++) hex += Math.floor(Math.random() * 16).toString(16)
  return hex
}

const containsBoundary = (parts: ReadonlyArray<MultipartPart>, boundary: string): boolean => {
  const marker = encoder.encode(`--${boundary}`)
  const newline = encoder.encode(CRLF)
  return parts.some((part) => {
    // Appended framing can complete a delimiter at the end of a payload.
    const data = part.body
    for (let i = data.indexOf(marker[0]!); i >= 0; i = data.indexOf(marker[0]!, i + 1)) {
      if (
        (i === 0 || matches(data, newline, i - newline.length)) &&
        (delimiterAt(data, marker, newline, i) ||
          (i + marker.length === data.length && matches(data, marker, i)))
      )
        return true
    }
    return false
  })
}

const validateHeaderValues = (part: MultipartPart): void => {
  if (!/^[\x21-\x7e]+$/.test(part.name) || /["\\]/.test(part.name)) {
    throw new MultipartCodecError("invalid part name")
  }
  if (!/^[\x20-\x7e]+$/.test(part.contentType) || part.contentType.trim() !== part.contentType) {
    throw new MultipartCodecError("invalid Content-Type")
  }
}

/** Encode once, choosing a boundary that cannot frame a body. @since 1.5.0 @category operations */
export const encodeMultipart = (
  parts: ReadonlyArray<MultipartPart>,
): {
  readonly data: Uint8Array
  readonly boundary: string
} => {
  for (const part of parts) validateHeaderValues(part)
  let boundary = generateBoundary()
  while (containsBoundary(parts, boundary)) boundary = generateBoundary()
  const chunks: Array<Uint8Array> = []
  for (const part of parts) {
    chunks.push(
      encoder.encode(
        `--${boundary}${CRLF}Content-Type: ${part.contentType}${CRLF}Content-Disposition: attachment; name="${part.name}"${CRLF}${CRLF}`,
      ),
    )
    chunks.push(part.body, encoder.encode(CRLF))
  }
  chunks.push(encoder.encode(`--${boundary}--${CRLF}`))
  return { data: concat(chunks), boundary }
}

/** Decode complete delimiter lines, never trimming payload bytes. @since 1.5.0 @category operations */
export const decodeMultipart = (data: Uint8Array, boundary: string): Array<MultipartPart> => {
  if (!validBoundary(boundary)) throw new MultipartCodecError("invalid boundary")
  const marker = encoder.encode(`--${boundary}`)
  let start = 0
  if (matches(data, encoder.encode(CRLF), 0)) start = 2
  else if (data[0] === 10) start = 1
  if (!matches(data, marker, start))
    throw new MultipartCodecError("Multipart body does not start with boundary")
  const firstSuffix = start + marker.length
  const suffix =
    data[firstSuffix] === 45 && data[firstSuffix + 1] === 45 ? firstSuffix + 2 : firstSuffix
  const newline =
    matches(data, encoder.encode(CRLF), suffix) || (suffix === data.length && start === 2)
      ? encoder.encode(CRLF)
      : encoder.encode("\n")
  if (start !== 0 && start !== newline.length)
    throw new MultipartCodecError("inconsistent leading newline")
  let delimiter = delimiterAt(data, marker, newline, start)
  if (!delimiter) throw new MultipartCodecError("invalid opening boundary")
  const parts: Array<MultipartPart> = []
  const names = new Set<string>()
  while (!delimiter.closing) {
    let pos = delimiter.end
    let name: string | undefined
    let contentType: string | undefined
    // Headers have their own textual newline rules; body framing uses the first delimiter's newline.
    while (true) {
      const end = data.indexOf(10, pos)
      if (end < 0) throw new MultipartCodecError("could not find end of headers in part")
      const lineEnd = data[end - 1] === 13 ? end - 1 : end
      const bytes = data.subarray(pos, lineEnd)
      if (bytes.some((b) => b < 32 || b > 126))
        throw new MultipartCodecError("invalid ASCII header")
      const line = decoder.decode(bytes)
      pos = end + 1
      if (line === "") break
      if (!/^[\x20-\x7e]+$/.test(line) || /^\s/.test(line))
        throw new MultipartCodecError("invalid header")
      const colon = line.indexOf(":")
      if (colon < 0) throw new MultipartCodecError("invalid header")
      const key = line.slice(0, colon).toLowerCase()
      const value = line.slice(colon + 1).trim()
      if (key === "content-type") {
        if (contentType !== undefined) throw new MultipartCodecError("duplicate Content-Type")
        contentType = value
      } else if (key === "content-disposition") {
        if (name !== undefined) throw new MultipartCodecError("duplicate Content-Disposition")
        const match = /^attachment;\s*name="([^"\\]+)"$/i.exec(value)
        if (!match)
          throw new MultipartCodecError("part missing or invalid name in Content-Disposition")
        name = match[1]!
      } else throw new MultipartCodecError("unsupported header")
    }
    if (name === undefined || contentType === undefined)
      throw new MultipartCodecError("part missing name or Content-Type")
    if (names.has(name)) throw new MultipartCodecError(`Duplicate multipart part name: ${name}`)
    names.add(name)
    let next: ReturnType<typeof delimiterAt> = undefined
    let bodyEnd = data.indexOf(newline[0]!, pos)
    for (; bodyEnd >= 0; bodyEnd = data.indexOf(newline[0]!, bodyEnd + 1)) {
      if (matches(data, newline, bodyEnd)) {
        next = delimiterAt(data, marker, newline, bodyEnd + newline.length)
        if (next) break
      }
    }
    if (!next) throw new MultipartCodecError("missing closing boundary")
    const part = { name, contentType, body: data.slice(pos, bodyEnd) }
    validateHeaderValues(part)
    parts.push(part)
    delimiter = next
  }
  if (delimiter.end !== data.length) throw new MultipartCodecError("data after closing boundary")
  return parts
}

/** Extract a single valid boundary parameter. @since 1.5.0 @category utils */
export const extractBoundary = (mimeType: string): string | null => {
  const sections = mimeType.split(";")
  if (sections.shift()?.trim().toLowerCase() !== "multipart/mixed") return null
  if (sections.length !== 1) return null
  const match = /^\s*boundary\s*=\s*(?:"([^"\\]*)"|([^\s"=]+))\s*$/i.exec(sections[0]!)
  if (!match) return null
  if (match[2] !== undefined && !/^[A-Za-z0-9'+_.-]+$/.test(match[2])) return null
  const boundary = match[1] ?? match[2]!
  return validBoundary(boundary) ? boundary : null
}
