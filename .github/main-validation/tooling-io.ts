import { Data, Effect, Schema } from "effect"
import { createHash, randomUUID } from "node:crypto"
import { createReadStream, createWriteStream } from "node:fs"
import * as fs from "node:fs/promises"
import { basename, dirname, join } from "node:path"
import { pipeline } from "node:stream/promises"
import { createGzip } from "node:zlib"

export class ValidationError extends Data.TaggedError("ValidationError")<{ message: string }> {}
export class ToolingIoError extends Data.TaggedError("ToolingIoError")<{
  message: string
  code: string | undefined
  cause: { readonly kind: "SystemError" | "InvalidData"; readonly code: string | undefined }
}> {}

// Never retain parser input, subprocess output, paths, or arbitrary exception messages.
export function errno(error: unknown): string | undefined {
  if (
    error !== null && typeof error === "object" && "code" in error &&
    typeof error.code === "string" && /^E[A-Z0-9]+$/.test(error.code)
  ) return error.code
  return undefined
}
export const io = <A>(body: () => Promise<A>) =>
  Effect.tryPromise({
    try: body,
    catch: (error) => {
      if (error instanceof ValidationError || error instanceof ToolingIoError) return error
      const code = errno(error)
      return new ToolingIoError({
        message: `I/O or parsing failure (${code ?? "invalid data"})`,
        code,
        cause: { kind: code ? "SystemError" : "InvalidData", code }
      })
    }
  })

export function decode<S extends Schema.Top & { readonly DecodingServices: never }>(
  schema: S,
  value: unknown,
  message: string
): S["Type"] {
  try {
    return Schema.decodeUnknownSync(schema)(value)
  } catch {
    throw new ValidationError({ message })
  }
}
export const hash = (bytes: Uint8Array | string) => createHash("sha256").update(bytes).digest("hex")
export const digest = async (path: string) => {
  const hash = createHash("sha256")
  for await (const chunk of createReadStream(path)) hash.update(chunk)
  return hash.digest("hex")
}

export async function publish(path: string, encoded: string): Promise<void> {
  await fs.mkdir(dirname(path), { recursive: true })
  const temporary = join(dirname(path), `.${basename(path)}.${randomUUID()}.partial`)
  try {
    const file = await fs.open(temporary, "wx")
    try {
      await file.writeFile(encoded)
      await file.sync()
    } finally {
      await file.close()
    }
    await fs.rename(temporary, path)
  } finally {
    await fs.rm(temporary, { force: true })
  }
}

export const save = (path: string, value: unknown) => publish(path, JSON.stringify(value) + "\n")

export async function compress(path: string): Promise<string> {
  const output = path + ".gz", temporary = output + ".partial"
  try {
    await pipeline(createReadStream(path), createGzip(), createWriteStream(temporary))
    await fs.rename(temporary, output)
    await fs.unlink(path)
    return output
  } finally {
    await fs.rm(temporary, { force: true })
  }
}

export function object(value: unknown): Record<string, unknown> {
  return decode(Schema.Record(Schema.String, Schema.Unknown), value, "expected an object")
}

// JSON.parse accepts duplicate keys. Metadata is a commit contract, not last-key-wins input.
export function uniqueJson(text: string): unknown {
  const parse = (input: string): unknown => {
    try {
      return JSON.parse(input)
    } catch {
      throw new ValidationError({ message: "invalid JSON" })
    }
  }
  let index = 0
  const whitespace = () => {
    while (/\s/.test(text[index] ?? "") && index < text.length) index++
  }
  const string = (): string => {
    const start = index++
    while (index < text.length) {
      const char = text[index++]
      if (char === "\\") index++
      else if (char === "\"") return decode(Schema.String, parse(text.slice(start, index)), "invalid JSON string")
    }
    throw new ValidationError({ message: "unterminated JSON string" })
  }
  const value = (): void => {
    whitespace()
    if (text[index] === "{") {
      index++
      whitespace()
      const keys = new Set<string>()
      if (text[index] !== "}") {
        while (true) {
          whitespace()
          if (text[index] !== "\"") throw new ValidationError({ message: "invalid JSON key" })
          const key = string()
          if (keys.has(key)) throw new ValidationError({ message: "duplicate metadata key" })
          keys.add(key)
          whitespace()
          if (text[index++] !== ":") throw new ValidationError({ message: "invalid JSON object" })
          value()
          whitespace()
          if (text[index] !== ",") break
          index++
        }
      }
      if (text[index++] !== "}") throw new ValidationError({ message: "invalid JSON object" })
    } else if (text[index] === "[") {
      index++
      whitespace()
      if (text[index] !== "]") {
        while (true) {
          value()
          whitespace()
          if (text[index] !== ",") break
          index++
        }
      }
      if (text[index++] !== "]") throw new ValidationError({ message: "invalid JSON array" })
    } else if (text[index] === "\"") string()
    else {
      const start = index
      while (index < text.length && !/[\s,}\]]/.test(text[index]!)) index++
      if (start === index) throw new ValidationError({ message: "invalid JSON value" })
    }
  }
  value()
  whitespace()
  if (index !== text.length) throw new ValidationError({ message: "trailing JSON input" })
  return parse(text)
}
