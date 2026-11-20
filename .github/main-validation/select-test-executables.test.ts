import { expect, test } from "bun:test"
import { chmodSync, mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs"
import { join, resolve } from "node:path"
import { readMessages, selectExecutables, targets, usableExecutables } from "./select-test-executables.ts"

const root = resolve(import.meta.dirname, "../..")
function artifact(label: keyof typeof targets, executable: string | null) {
  const target = targets[label]
  return {
    reason: "compiler-artifact",
    profile: { test: true },
    target: {
      name: target.name,
      kind: [...target.kind],
      src_path: join(root, target.source)
    },
    executable
  }
}
test("selects only requested Cargo targets and deduplicates identical messages", () => {
  const lib = artifact("lib", "lib"), integration = artifact("integration", "integration")
  expect(selectExecutables([integration, lib, lib], root, ["lib"])).toEqual({ lib: "lib" })
  expect(selectExecutables([integration, lib], root, ["lib", "integration"])).toEqual({
    lib: "lib",
    integration: "integration"
  })
})
test("rejects absent, ambiguous, non-test, wrong package and wrong target artifacts", () => {
  const lib = artifact("lib", "lib")
  for (
    const messages of [
      [],
      [lib, artifact("lib", "other")],
      [{ ...lib, profile: { test: false } }],
      [artifact("lib", null)],
      [{ ...lib, reason: "build-script-executed" }],
      [{ ...lib, target: { ...lib.target, src_path: join(root, "other-package/src/lib.rs") } }],
      [{ ...lib, target: { ...lib.target, name: "other" } }],
      [{ ...lib, target: { ...lib.target, kind: ["bin"] } }]
    ]
  ) {
    expect(() => selectExecutables(messages, root, ["lib"])).toThrow("exactly one lib")
  }
  for (const request of [[], ["lib", "lib"], ["unknown"], ["toString"]]) {
    expect(() => selectExecutables([], root, request)).toThrow()
  }
})
test("feature variants remain ambiguous instead of guessing a binary by its name", () => {
  const messages = [
    { ...artifact("lib", "default-features"), features: ["default"] },
    { ...artifact("lib", "other-features"), features: ["default", "extra"] }
  ]
  expect(() => selectExecutables(messages, root, ["lib"])).toThrow("exactly one lib")
})
test("Cargo boundaries reject malformed field types instead of trusting assertions or truthiness", () => {
  const lib = artifact("lib", "lib")
  for (
    const message of [null, [], { ...lib, executable: 42 }, { ...lib, profile: { test: "true" } }, {
      ...lib,
      target: { ...lib.target, kind: "lib" }
    }, { ...lib, target: { ...lib.target, src_path: [] } }]
  ) {
    expect(() => selectExecutables([message], root, ["lib"])).toThrow("invalid Cargo message")
  }
})
test("CLI requires existing executable files, rejects malformed JSON and ignores newer XFS decoys", () => {
  const scratch = join(root, ".amp/in/orb-backup/selector-tests")
  mkdirSync(scratch, { recursive: true })
  const directory = mkdtempSync(join(scratch, "case-"))
  try {
    const binary = join(directory, "actual binary"), messages = join(directory, "messages")
    writeFileSync(binary, "#!/bin/sh\nexit 0\n", { mode: 0o755 })
    for (const name of ["golem_worker_executor-stale-decoy", "integration-stale-decoy"]) {
      writeFileSync(join(directory, name), "decoy", { mode: 0o755 })
      utimesSync(join(directory, name), new Date("2038-01-01"), new Date("2038-01-01"))
    }
    const emit = (path: string) => writeFileSync(messages, `\n${JSON.stringify(artifact("lib", path))}\n`)
    emit(binary)
    expect(usableExecutables(messages, root, ["lib"])).toEqual({ lib: binary })
    const cli = Bun.spawnSync(["bun", join(import.meta.dirname, "select-test-executables.ts"), messages, root, "lib"])
    expect(cli.exitCode).toBe(0)
    expect(cli.stdout.toString()).toBe(`lib\t${binary}\n`)
    chmodSync(binary, 0o644)
    expect(() => usableExecutables(messages, root, ["lib"])).toThrow("non-executable")
    emit(join(directory, "missing"))
    expect(() => usableExecutables(messages, root, ["lib"])).toThrow("missing")
    emit(directory)
    expect(() => usableExecutables(messages, root, ["lib"])).toThrow("non-executable")
    writeFileSync(messages, "\nnot json\n")
    expect(() => readMessages(messages)).toThrow("line 2")
    expect(
      Bun.spawnSync(["bun", join(import.meta.dirname, "select-test-executables.ts"), messages, root, "lib"]).exitCode
    ).toBe(1)
    for (
      const text of [
        "{\"reason\":\"secret-marker\",\"reason\":\"compiler-artifact\"}",
        "{\"executable\":{\"secret-marker\":true}}",
        "\"secret-marker\""
      ]
    ) {
      writeFileSync(messages, text)
      expect(() => readMessages(messages)).toThrow("line 1")
      const rejected = Bun.spawnSync([
        "bun",
        join(import.meta.dirname, "select-test-executables.ts"),
        messages,
        root,
        "lib"
      ])
      expect(rejected.exitCode).toBe(1)
      expect(rejected.stderr.toString()).not.toContain("secret-marker")
    }
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
})
