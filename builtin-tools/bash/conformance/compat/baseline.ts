import { FileSystem } from "@effect/platform"
import { Console, Effect } from "effect"
import { join } from "node:path"
import { baselineChanges, ValidationError } from "../model.ts"
import { attempt, here, main } from "../platform.ts"

export function failing(text: string) {
  // oxlint-disable-next-line eslint/no-control-regex -- Brush colors its terminal report with ANSI escapes.
  text = text.replace(/\x1b\[[0-9;]*m/g, "")
  if (!text.includes("test case(s) ran")) {
    throw new ValidationError({ message: "no summary line; the run did not finish" })
  }
  const reports = new Map<string, string>()
  for (const block of text.split(/\n(?=\* Test case: \[)/)) {
    const name = /^\* Test case: \[(.*?)\]\.\.\./.exec(block)?.[1]
    if (
      name !== undefined &&
      block.includes("\n    FAILED.") &&
      name !== "printf broken pipe returns 141 in PIPESTATUS"
    ) {
      reports.set(name, block.slice(0, block.indexOf("\n    FAILED.")).trimEnd())
    }
  }
  return reports
}
export const program = Effect.fn("conformance.baseline")(function*(argv: string[]) {
  const [command, output] = argv
  if (argv.length !== 2 || (command !== "check" && command !== "update") || output === undefined) {
    return yield* Effect.fail(
      new ValidationError({ message: "usage: baseline.ts check|update OUTPUT" })
    )
  }
  const fs = yield* FileSystem.FileSystem
  const text = yield* fs.readFileString(output)
  const now = yield* attempt(() => failing(text))
  const path = join(here, "compat", "baseline.txt")
  if (command === "update") {
    yield* fs.writeFileString(
      path,
      [...now.keys()]
        .sort()
        .map((n) => n + "\n")
        .join("")
    )
    yield* Console.log(`${now.size} failing cases recorded in baseline.txt`)
    return true
  }
  const known = new Set(
    (yield* fs.readFileString(path)).split(/\r?\n/).filter(Boolean)
  )
  const changes = baselineChanges(known, known, new Set(now.keys()))
  for (const name of changes.new) {
    yield* Console.log(`NEW FAILURE ${name}\n${now.get(name)}`)
  }
  for (const name of changes.fixed) {
    yield* Console.log(`NOW PASSES ${name} (remove it from baseline.txt)`)
  }
  yield* Console.log(
    `${now.size} failing, ${known.size} in the baseline; ${changes.new.length} new, ${changes.fixed.length} fixed`
  )
  return !changes.new.length && !changes.fixed.length
})
if (import.meta.main) main(program(process.argv.slice(2)))
