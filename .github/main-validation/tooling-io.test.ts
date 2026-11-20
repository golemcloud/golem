import { expect, test } from "bun:test"
import { Effect, Schema } from "effect"
import * as fs from "node:fs/promises"
import { join, resolve } from "node:path"
import { gunzipSync } from "node:zlib"
import {
  compress,
  decode,
  digest,
  errno,
  hash,
  io,
  publish,
  ToolingIoError,
  uniqueJson,
  ValidationError
} from "./tooling-io.ts"

test("callback I/O has a concrete, secret-safe error channel and preserves errno", async () => {
  const failure: Effect.Effect<never, ToolingIoError | ValidationError> = io(async () => {
    throw Object.assign(new Error("parser-secret-marker"), { code: "ENOENT", path: "path-secret-marker" })
  })
  const error = await Effect.runPromise(failure.pipe(Effect.flip))
  expect(error).toBeInstanceOf(ToolingIoError)
  if (error instanceof ToolingIoError) {
    expect(error.code).toBe("ENOENT")
    expect(error.cause).toEqual({ kind: "SystemError", code: "ENOENT" })
    expect(errno(error)).toBe("ENOENT")
  }
  expect(String(error)).not.toContain("secret-marker")
  expect(JSON.stringify(error)).not.toContain("secret-marker")
  for (const original of [new Error("secret-marker"), { code: "secret-marker" }, "secret-marker"]) {
    const safe = await Effect.runPromise(
      io(async () => {
        throw original
      }).pipe(Effect.flip)
    )
    expect(safe).toBeInstanceOf(ToolingIoError)
    expect(JSON.stringify(safe)).not.toContain("secret-marker")
  }
  const expected = new ValidationError({ message: "invalid snapshot" })
  expect(
    await Effect.runPromise(
      io(async () => {
        throw expected
      }).pipe(Effect.flip)
    )
  ).toBe(expected)
})

test("JSON and schema boundaries reject duplicate keys and malformed values without quoting input", () => {
  expect(uniqueJson("{\"outer\":[{\"value\":\"escaped\\\"string\"}],\"other\":null}")).toEqual({
    outer: [{ value: "escaped\"string" }],
    other: null
  })
  for (
    const text of [
      "{\"secret-marker\":1,\"secret-marker\":2}",
      "{\"a\":{\"x\":1,\"\\u0078\":2}}",
      "\"\\qsecret-marker\"",
      "secret-marker"
    ]
  ) {
    try {
      uniqueJson(text)
      throw new Error("accepted invalid JSON")
    } catch (error) {
      expect(error).toBeInstanceOf(ValidationError)
      expect(String(error)).not.toContain("secret-marker")
    }
  }
  expect(() => decode(Schema.Struct({ name: Schema.String }), { name: 42 }, "invalid name")).toThrow("invalid name")
})

test("atomic publication, streaming digests and compression preserve contents and clean partial files", async () => {
  const scratch = resolve(import.meta.dirname, "../../.amp/in/orb-backup/tooling-io-tests")
  await fs.mkdir(scratch, { recursive: true })
  const directory = await fs.mkdtemp(join(scratch, "case-"))
  try {
    const path = join(directory, "report.json")
    await publish(path, "first")
    await publish(path, "second")
    expect(await digest(path)).toBe(hash("second"))
    const compressed = await compress(path)
    expect(gunzipSync(await fs.readFile(compressed)).toString()).toBe("second")
    expect(await fs.readdir(directory)).toEqual(["report.json.gz"])
    await expect(compress(join(directory, "missing"))).rejects.toThrow()
    expect(await fs.readdir(directory)).toEqual(["report.json.gz"])
  } finally {
    await fs.rm(directory, { recursive: true, force: true })
  }
})
