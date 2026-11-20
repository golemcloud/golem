import { NodeContext } from "@effect/platform-node"
import { expect, test } from "bun:test"
import { Effect, Schema } from "effect"
import { join } from "node:path"
import { failing } from "./compat/baseline.ts"
import { argumentsFor, loadCases, oracleFingerprint } from "./conformance.ts"
import {
  baselineChanges,
  type Case,
  Corpus,
  credited,
  decodeBytes,
  differingField,
  encodeBytes,
  expectedFor,
  Golden,
  goldenFor,
  hash,
  type Outcome,
  redundantFixture,
  splitCalls,
  wasmStatus
} from "./model.ts"
import { here, readJson } from "./platform.ts"

const c: Case = {
  name: "c",
  script: "echo hi",
  corpus: "corpus",
  tier: "pr",
  tags: [],
  options: {}
}
const result: Outcome = {
  status: 7,
  stdout: { text: "out\n" },
  stderr: { base64: "/wBlcnI=" }
}
const fingerprint = { dockerfile: "docker", profile: "profile" }
const golden = {
  oracle: fingerprint,
  cases: new Map([["c", { ...result, script: hash(c.script) }]])
}

test("exact comparison preserves status, diagnostics, binary data and UTF-8 BOM", () => {
  expect(differingField(result, { ...result })).toBeUndefined()
  expect(differingField(result, { ...result, status: 1 })).toBe("status")
  expect(differingField(result, { ...result, stdout: { text: "out" } })).toBe(
    "stdout"
  )
  expect(differingField(result, { ...result, stderr: { text: "" } })).toBe(
    "stderr"
  )
  expect(differingField({ ...result, stderr: { text: "" } }, result)).toBe(
    "stderr"
  )
  for (
    const bytes of [
      Buffer.from([255, 254, 0, 10]),
      Buffer.from("\ufeffhéllo\n"),
      Buffer.from("normal\n")
    ]
  ) {
    expect(decodeBytes(encodeBytes(bytes))).toEqual(bytes)
  }
})

test("WASI status must be a valid u8 and agree with the report, not just success/failure", () => {
  expect(wasmStatus(127, "127\n")).toBe(127)
  expect(wasmStatus(0, "0")).toBe(0)
  for (
    const [status, report] of [
      [1, "2"],
      [3, "2"],
      [1, "0"],
      [0, ""],
      [0, "256"],
      [0, "-1"],
      [0, "1.0"]
    ] as const
  ) {
    expect(() => wasmStatus(status, report)).toThrow()
  }
})

test("goldens reject a missing case, edited script and changed oracle", () => {
  expect(goldenFor(c, golden, fingerprint)).toEqual(result)
  expect(() => goldenFor({ ...c, script: "echo new" }, golden, fingerprint)).toThrow("older script")
  expect(() => goldenFor(c, golden, { ...fingerprint, profile: "new" })).toThrow("another oracle")
  expect(() => goldenFor(c, golden, { ...fingerprint, dockerfile: "new" })).toThrow("another oracle")
  expect(() => goldenFor(c, { ...golden, cases: new Map() }, fingerprint)).toThrow("no golden")
})

test("stderr fixtures preserve oracle status/stdout and redundant fixtures fail", () => {
  const fixture = {
    ...c,
    expectedStderr: { value: { text: "ours\n" }, reason: "deliberate" }
  }
  expect(expectedFor(fixture, result)).toEqual({
    ...result,
    stderr: { text: "ours\n" }
  })
  expect(redundantFixture(fixture, result)).toBeUndefined()
  expect(
    redundantFixture(fixture, { ...result, stderr: { text: "ours\n" } })
  ).toContain("remove")
  expect(
    redundantFixture({ ...c, expected: { ...result, reason: "why" } }, result)
  ).toContain("remove")
})

test("corpus validation requires reasons and typed options", () => {
  const decode = Schema.decodeUnknownSync(Corpus, {
    onExcessProperty: "error"
  })
  const valid = {
    tier: "pr",
    description: "",
    cases: [{ name: "a", script: "true", tags: ["error"], options: {} }]
  }
  expect(decode(valid).cases.map((c) => c.tags)).toEqual([["error"]])
  for (
    const extra of [
      { expected: { ...result } },
      { oracleRace: "" },
      { options: { timeout: 0 } },
      { expectedStderr: { value: { text: "x" } } }
    ]
  ) {
    expect(() => decode({ ...valid, cases: [{ ...valid.cases[0], ...extra }] })).toThrow()
  }
})

test("call markers need their own line and carry only the declared oracle cwd", () => {
  expect(splitCalls("cd /tmp\n#--call-- /tmp\npwd\n#--call--\npwd")).toEqual([
    { cwd: "/", script: "cd /tmp" },
    { cwd: "/tmp", script: "pwd" },
    { cwd: "/", script: "pwd" }
  ])
  const script = "echo '#--call--'\n  #--call--\n#--call--x"
  expect(splitCalls(script)).toEqual([{ cwd: "/", script }])
})

test("feature detectors credit syntax and nested command starts, and reject unknown tags", () => {
  const credit = credited({
    ...c,
    name: "grep: missing operand",
    tags: ["unknown"],
    script: "if true; then while getopts a x; do echo ${a[0]#x}; done; fi"
  })
  expect([...credit.found]).toContain("builtin.getopts")
  expect([...credit.found]).toContain("expansion.parameter.trim")
  expect([...credit.found]).toContain("cmd.grep.error")
  expect(credit.unknown).toEqual(["unknown"])
  const inheritedTags = [
    "__proto__",
    "constructor",
    "toString",
    "hasOwnProperty"
  ]
  expect(credited({ ...c, tags: inheritedTags }).unknown).toEqual(
    inheritedTags
  )
  expect(
    credited({ ...c, script: "if [ true ]; then :; fi" }).found.has(
      "builtin.["
    )
  ).toBe(true)
  expect(
    credited({ ...c, script: "café() { echo yes; }" }).found.has(
      "compound.function"
    )
  ).toBe(true)
})

test("failure baselines reject new failures and fixes but ignore unselected known cases", () => {
  expect(
    baselineChanges(
      new Set(["old", "fixed", "unselected"]),
      new Set(["old", "fixed", "new"]),
      new Set(["old", "new"])
    )
  ).toEqual({ new: ["new"], fixed: ["fixed"] })
})

test("Brush reports require completion, preserve diagnostics and exclude only the named race", () => {
  const output =
    "* Test case: [bad]...\n    diagnostic\n    FAILED.\n* Test case: [good]...\n    PASSED.\n* Test case: [printf broken pipe returns 141 in PIPESTATUS]...\n    FAILED.\n3 test case(s) ran"
  expect([...failing("\x1b[31m" + output + "\x1b[0m")]).toEqual([
    ["bad", "* Test case: [bad]...\n    diagnostic"]
  ])
  expect(() => failing("* Test case: [bad]...\n    FAILED.")).toThrow(
    "did not finish"
  )
})

test("jobs honor the executor budget and explicit override", () => {
  const old = process.env.BASH_CONFORMANCE_JOBS
  try {
    process.env.BASH_CONFORMANCE_JOBS = "4"
    expect(argumentsFor(["check", "--wasm", "fixture.wasm"]).jobs).toBe(4)
    expect(
      argumentsFor(["check", "--wasm", "fixture.wasm", "--jobs", "7"]).jobs
    ).toBe(7)
    expect(() => argumentsFor(["check", "--wasm", "fixture.wasm", "--jobs", "0"])).toThrow()
  } finally {
    if (old === undefined) delete process.env.BASH_CONFORMANCE_JOBS
    else process.env.BASH_CONFORMANCE_JOBS = old
  }
})

test("every corpus keeps a current golden and valid feature tags", async () => {
  await Effect.runPromise(
    Effect.gen(function*() {
      const cases = yield* loadCases(),
        fingerprint = yield* oracleFingerprint
      const corpora = new Map<string, typeof Golden.Type>()
      for (const corpus of new Set(cases.map((c) => c.corpus))) {
        corpora.set(
          corpus,
          yield* readJson(join(here, "goldens", `${corpus}.json`), Golden)
        )
      }
      for (const c of cases) {
        expect(() => goldenFor(c, corpora.get(c.corpus)!, fingerprint)).not.toThrow()
        expect(credited(c).unknown).toEqual([])
      }
    }).pipe(Effect.provide(NodeContext.layer))
  )
}, 120000)
