import { Schema } from "effect"
import { createReadStream } from "node:fs"
import * as fs from "node:fs/promises"
import { availableParallelism, loadavg } from "node:os"
import { createGunzip } from "node:zlib"

const stored = Schema.decodeUnknownSync(Schema.Struct({ status: Schema.Literal("stored") }))

export async function resourceSample() {
  const cpu = (await fs.readFile("/proc/stat", "utf8")).trim().split(/\r?\n/, 1).join("").split(/\s+/).slice(1, 9).map(
    Number
  )
  const memory: Record<string, number> = {}
  for (const line of (await fs.readFile("/proc/meminfo", "utf8")).split("\n")) {
    const [name, value] = line.split(":")
    if (
      name !== undefined && value !== undefined && ["MemTotal", "MemAvailable", "SwapTotal", "SwapFree"].includes(name)
    ) memory[name] = Number(value.trim().split(/\s+/)[0]) * 1024
  }
  const disks: Record<string, { total_bytes: number; free_bytes: number }> = {}
  for (
    const [name, path] of [["worktree", process.cwd()], ["target", process.env.CARGO_TARGET_DIR ?? "target"]] as const
  ) {
    try {
      const stat = await fs.statfs(path)
      disks[name] = { total_bytes: stat.blocks * stat.bsize, free_bytes: stat.bavail * stat.bsize }
    } catch (error) {
      if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) throw error
    }
  }
  const pressure: Record<string, string> = {}
  for (const name of ["cpu", "memory", "io"]) {
    try {
      pressure[name] = (await fs.readFile(`/proc/pressure/${name}`, "utf8")).trim()
    } catch { /* Optional kernel evidence. */ }
  }
  // Linux exposes /proc/stat counters in USER_HZ (100), not kernel CONFIG_HZ.
  return {
    timestamp: new Date().toISOString(),
    monotonic_seconds: performance.now() / 1000,
    scope: "runner-host",
    available_cpus: availableParallelism(),
    clock_ticks_per_second: 100,
    cpu_ticks: Object.fromEntries(
      ["user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal"].map((name, index) => [name, cpu[index]])
    ),
    memory_bytes: memory,
    load_average: loadavg(),
    disks,
    pressure,
    diskstats: await fs.readFile("/proc/diskstats", "utf8")
  }
}

export async function failureExcerpt(path: string): Promise<void> {
  try {
    const stream = path.endsWith(".gz") ? createReadStream(path).pipe(createGunzip()) : createReadStream(path)
    let tail = Buffer.alloc(0)
    for await (const chunk of stream) tail = Buffer.concat([tail, Buffer.from(chunk as Uint8Array)]).subarray(-8192)
    for (const line of tail.toString("utf8").trimEnd().split("\n").slice(-15)) {
      console.log(`[log excerpt] ${line.slice(0, 500)}`)
    }
  } catch {
    console.log("[validation] Log excerpt unavailable; inspect retained evidence")
  }
}

export async function publishProgress(progress: unknown, createdAt: string): Promise<void> {
  const token = process.env.CI_DASHBOARD_PROGRESS_TOKEN, address = process.env.CI_DASHBOARD_URL
  if (!address || !token) return
  try {
    const url = new URL(address)
    if (url.protocol !== "https:" || !url.hostname || url.username || url.password || url.search || url.hash) {
      throw new Error("invalid dashboard URL")
    }
    const body = JSON.stringify({
      run_id: Number(process.env.GITHUB_RUN_ID),
      attempt: Number(process.env.GITHUB_RUN_ATTEMPT),
      created_at: createdAt,
      source: process.env.MAIN_VALIDATION_SOURCE,
      workflow_sha: process.env.GITHUB_SHA,
      branch: process.env.GITHUB_REF_NAME,
      event: process.env.GITHUB_EVENT_NAME,
      context: process.env.MAIN_VALIDATION_CONTEXT,
      progress
    })
    if (body.includes(JSON.stringify(token).slice(1, -1))) throw new Error("credential in publication")
    for (
      const key of [
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
        "GITHUB_SHA",
        "GITHUB_REF_NAME",
        "GITHUB_EVENT_NAME",
        "MAIN_VALIDATION_SOURCE",
        "MAIN_VALIDATION_CONTEXT"
      ]
    ) if (!process.env[key]) throw new Error("missing Actions identity")
    const response = await fetch(address.replace(/\/+$/, "") + "/api/progress", {
      method: "POST",
      body,
      redirect: "error",
      signal: AbortSignal.timeout(5000),
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" }
    })
    const reader = response.body?.getReader()
    if (!reader) throw new Error("missing confirmation")
    let bytes = Buffer.alloc(0)
    try {
      while (true) {
        const part = await reader.read()
        if (part.done) break
        bytes = Buffer.concat([bytes, part.value])
        if (bytes.length > 1024) throw new Error("oversize confirmation")
      }
    } finally {
      await reader.cancel()
    }
    if (response.status !== 200) throw new Error("unconfirmed publication")
    stored(JSON.parse(bytes.toString()))
  } catch {
    console.log("[validation] Dashboard publication failed; local progress and logs are retained")
  }
}
