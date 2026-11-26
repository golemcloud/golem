import { Effect, Schema } from "effect"
import { mkdir, readFile, rename, stat, statfs, writeFile } from "node:fs/promises"
import { join, resolve } from "node:path"
import { decode, errno, io, ValidationError } from "./tooling-io.ts"

const Cycle = Schema.Struct({
  run_id: Schema.String,
  attempt: Schema.String,
  source: Schema.String,
  workflow_sha: Schema.String,
  recorded_at: Schema.String,
  workload_exit_code: Schema.NullOr(Schema.String)
})
type Cycle = typeof Cycle.Type

// A cycle marker identifies visible contents; it is not a provider commit receipt.
export const cacheDiagnostics = (
  phase: "begin" | "complete",
  root: string,
  report: string,
  environment: NodeJS.ProcessEnv = process.env
) =>
  io(async () => {
    const cycle: Cycle = {
      run_id: environment.GITHUB_RUN_ID ?? "",
      attempt: environment.GITHUB_RUN_ATTEMPT ?? "",
      source: environment.MAIN_VALIDATION_SOURCE ?? "",
      workflow_sha: environment.GITHUB_SHA ?? "",
      recorded_at: new Date().toISOString(),
      workload_exit_code: environment.WORKLOAD_EXIT_CODE ?? null
    }
    const disks = []
    for (
      const [name, relative, key] of [
        ["compiler", "target", "main-validation-linux-target"],
        ["dependencies", ".validation-cache", "main-validation-linux-dependencies"]
      ] as const
    ) {
      const path = join(root, relative)
      const marker = join(path, ".main-validation-cycle.json")
      const [capacity, identity] = await Promise.all([statfs(path), stat(path)])
      let previous: unknown = null
      try {
        const fields = decode(Cycle, JSON.parse(await readFile(marker, "utf8")), "invalid cycle marker")
        previous = Object.fromEntries(
          Object.entries(fields).map(([field, value]) => [field, value === null ? null : value.slice(0, 100)])
        )
      } catch (error) {
        if (errno(error) !== "ENOENT") {
          previous = { warning: "Cycle marker unreadable" }
        }
      }
      disks.push({
        name,
        key,
        device: identity.dev,
        total_bytes: capacity.blocks * capacity.bsize,
        free_bytes: capacity.bavail * capacity.bsize,
        previous_cycle: previous
      })
      if (phase === "complete") {
        await writeFile(marker + ".pending", JSON.stringify(cycle))
        await rename(marker + ".pending", marker)
      }
    }
    const evidence = {
      phase,
      cycle,
      disks,
      durability: "Commit is requested later by sticky-disk post hooks; not confirmed here"
    }
    await mkdir(report, { recursive: true })
    await writeFile(join(report, `cache-${phase}.json`), JSON.stringify(evidence, null, 2) + "\n")
    console.log(JSON.stringify(evidence))
  })

if (import.meta.main) {
  const phase = process.argv[2]
  if (phase !== "begin" && phase !== "complete") throw new ValidationError({ message: "Expected begin or complete" })
  await Effect.runPromise(
    cacheDiagnostics(
      phase,
      process.cwd(),
      resolve(process.env.MAIN_VALIDATION_REPORT_DIR ?? "target/main-validation-reports")
    )
  )
}
