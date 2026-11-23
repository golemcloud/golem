import { expect, test } from "bun:test"
import { Effect } from "effect"
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { join, resolve } from "node:path"
import { cacheDiagnostics } from "./cache-diagnostics.ts"

test("cache evidence distinguishes a visible prior cycle from requested persistence", async () => {
  const scratch = resolve(import.meta.dir, "../../.amp/in/orb-backup/cache-diagnostic-tests")
  await mkdir(scratch, { recursive: true })
  const root = await mkdtemp(join(scratch, "cycle-"))
  try {
    for (const path of ["target", ".validation-cache"]) await mkdir(join(root, path))
    const report = join(root, "reports")
    await Effect.runPromise(cacheDiagnostics("begin", root, report, { GITHUB_RUN_ID: "1" }))
    const begin = JSON.parse(await readFile(join(report, "cache-begin.json"), "utf8"))
    expect(begin.disks.map((disk: { previous_cycle: unknown }) => disk.previous_cycle)).toEqual([null, null])
    await Effect.runPromise(cacheDiagnostics("complete", root, report, { GITHUB_RUN_ID: "1", WORKLOAD_EXIT_CODE: "7" }))
    await Effect.runPromise(cacheDiagnostics("begin", root, report, { GITHUB_RUN_ID: "2" }))
    const next = JSON.parse(await readFile(join(report, "cache-begin.json"), "utf8"))
    expect(next.disks[0].previous_cycle.run_id).toBe("1")
    expect(next.disks[1].previous_cycle.workload_exit_code).toBe("7")
    expect(next.durability).toContain("not confirmed")
    await writeFile(join(root, "target/.main-validation-cycle.json"), "not json")
    await Effect.runPromise(cacheDiagnostics("begin", root, report, { GITHUB_RUN_ID: "3" }))
    expect(JSON.parse(await readFile(join(report, "cache-begin.json"), "utf8")).disks[0].previous_cycle.warning).toBe(
      "Cycle marker unreadable"
    )
    for (const value of [null, [], { run_id: { secret: "secret-marker" } }]) {
      await writeFile(join(root, "target/.main-validation-cycle.json"), JSON.stringify(value))
      await Effect.runPromise(cacheDiagnostics("begin", root, report))
      const text = await readFile(join(report, "cache-begin.json"), "utf8")
      expect(JSON.parse(text).disks[0].previous_cycle.warning).toBe("Cycle marker unreadable")
      expect(text).not.toContain("secret-marker")
    }
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
