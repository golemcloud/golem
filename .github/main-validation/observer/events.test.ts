import { afterEach, beforeEach, expect, test } from "bun:test"
import { Effect } from "effect"
import { createHmac } from "node:crypto"
import * as fs from "node:fs/promises"
import { resolve } from "node:path"
import { configure } from "./configure-webhook.ts"
import { atomicJson, checkedCommand, object } from "./runtime.ts"
import { run, scratch, write } from "./test-helpers.ts"
let directory: string, state: string, config: string
beforeEach(async () => {
  directory = await scratch("events")
  state = resolve(directory, "state")
  config = resolve(directory, ".local/main-validation-webhook.json")
  for (const name of ["seen", "events"]) await fs.mkdir(resolve(state, name), { recursive: true })
})
afterEach(async () => {
  await fs.rm(directory, { recursive: true, force: true })
})
test("process acquisition and JSON write failures stay typed and release temporary files", async () => {
  expect((await run(Effect.result(checkedCommand(["observer-fixture-command-does-not-exist"]))))._tag).toBe("Failure")
  const output = resolve(directory, "output")
  expect((await run(Effect.result(atomicJson(resolve(output, "record.json"), { invalid: 1n }))))._tag).toBe("Failure")
  expect(await fs.readdir(output)).toEqual([])
})
test("observer subprocess deadlines finish even when the child ignores SIGTERM", async () => {
  const pidFile = resolve(directory, "child-pid")
  const child = run(
    Effect.result(
      checkedCommand([
        process.execPath,
        "-e",
        "process.on(\"SIGTERM\", () => {}); await Bun.write(process.argv[1], String(process.pid)); setInterval(() => {}, 1000)",
        pidFile
      ], { timeout: 0.3 })
    )
  )
  try {
    const result = await Promise.race([child, Bun.sleep(2000).then(() => "cleanup-hung")])
    expect(result).not.toBe("cleanup-hung")
    expect(typeof result === "object" && result._tag).toBe("Failure")
  } finally {
    try {
      process.kill(Number(await fs.readFile(pidFile, "utf8")), "SIGKILL")
    } catch (error) {
      expect(["ENOENT", "ESRCH"]).toContain(String(object(error).code))
    }
    await child
  }
})
test("event inbox orders run/attempt, excludes seen and no-selection, and does not mutate event ownership", async () => {
  for (
    const [id, attempt, status] of [[91, 2, "notified"], [90, 1, "notified"], [91, 1, "pending"], [
      89,
      1,
      "no-selection"
    ], [88, 1, "seen"]]
  ) await write(resolve(state, "events", `${id}-a${attempt}.json`), { run: id, attempt, status })
  await write(resolve(state, "seen/90-a1.json"), {})
  const original = await fs.readFile(resolve(state, "events/91-a1.json"), "utf8")
  const output = await run(
    checkedCommand(["bash", resolve(import.meta.dir, "list-runs.sh"), "--events"], {
      env: { ...process.env, OBSERVER_STATE_DIR: state }
    })
  )
  expect(output).toBe("91\t1\n91\t2\n")
  expect(await fs.readFile(resolve(state, "events/91-a1.json"), "utf8")).toBe(original)
})
test("private init retains existing owner/config; process command uses same contract", async () => {
  await run(configure(["init", "T-existing-owner", state], directory))
  const before = await fs.readFile(config, "utf8"), value = object(JSON.parse(before))
  expect((await fs.stat(config)).mode & 0o777).toBe(0o600)
  expect(value.stateDir).toBe(state)
  expect(String(value.secret)).toHaveLength(64)
  await expect(run(configure(["init", "T-other-owner", state], directory))).rejects.toThrow("Configuration exists")
  expect(await fs.readFile(config, "utf8")).toBe(before)
  const child = Bun.spawn(["bun", resolve(import.meta.dir, "configure-webhook.ts"), "init", "T-other-owner", state], {
    cwd: directory,
    env: { PATH: process.env.PATH },
    stderr: "pipe",
    stdout: "pipe"
  })
  expect(await child.exited).toBe(1)
  expect(await new Response(child.stderr).text()).not.toContain(String(value.secret))
})
test("registration reuses existing hook after ambiguous create, refuses incompatible transport or ambiguous inventory", async () => {
  await fs.mkdir(resolve(directory, ".local"))
  const value = { url: "https://fixture.invalid/capability", secret: "fixture-only", stateDir: state }
  await write(config, value)
  const hook = {
    id: 17,
    active: true,
    events: ["workflow_run"],
    config: { url: value.url, content_type: "json", insecure_ssl: "0" }
  }
  const calls: string[] = []
  const api = (path: string, payload?: unknown) => {
    calls.push(path)
    expect(payload).toBeUndefined()
    return Effect.succeed([hook])
  }
  await run(configure(["register"], directory, api))
  expect(calls).toEqual(["hooks?per_page=100"])
  expect(object(JSON.parse(await fs.readFile(config, "utf8"))).hookId).toBe(17)
  for (const [content_type, insecure_ssl] of [["form", "0"], ["json", "1"]]) {
    const invalid = { ...hook, config: { ...hook.config, content_type, insecure_ssl } }
    await expect(run(configure(["register"], directory, () => Effect.succeed([invalid])))).rejects.toThrow(
      "different settings"
    )
  }
  await expect(run(configure(["register"], directory, () => Effect.succeed([hook, hook])))).rejects.toThrow("Ambiguous")
  await expect(run(configure(["register"], directory, () => Effect.succeed(Array.from({ length: 100 }, () => ({}))))))
    .rejects.toThrow("Ambiguous")
})
test("unconfirmed webhook creation fails normally without retry or replacing private configuration", async () => {
  await fs.mkdir(resolve(directory, ".local"))
  await write(config, {
    url: "https://fixture.invalid/capability",
    secret: "fixture-only",
    stateDir: state,
    ownerThread: "T-existing-owner"
  })
  const before = await fs.readFile(config, "utf8"), calls: string[] = []
  const result = await run(Effect.result(configure(["register"], directory, (path) => {
    calls.push(path)
    return path.startsWith("hooks?") ? Effect.succeed([]) : Effect.fail(new Error("fixture disconnect after creation"))
  })))
  expect(result._tag).toBe("Failure")
  expect(calls).toEqual(["hooks?per_page=100", "hooks"])
  expect(await fs.readFile(config, "utf8")).toBe(before)
})
test("signed probe requires already seen real attempt and uses two distinct delivery IDs, without altering seen state", async () => {
  await fs.mkdir(resolve(directory, ".local"))
  await write(config, { stateDir: state, secret: "fixture-only", url: "https://fixture.invalid" })
  const api = () => Effect.succeed({ path: ".github/workflows/main-validation.yaml", status: "completed" })
  let calls = 0
  const ids = new Set<string>()
  const send = (_url: string, init?: RequestInit) => {
    calls++
    const headers = new Headers(init?.headers)
    expect(headers.get("X-Hub-Signature-256")).toBe(
      "sha256=" + createHmac("sha256", "fixture-only").update(String(init?.body)).digest("hex")
    )
    ids.add(headers.get("Idempotency-Key")!)
    return Effect.succeed({ status: 200, body: "" })
  }
  await expect(run(configure(["probe", "123", "2"], directory, api, send))).rejects.toThrow("already processed")
  expect(calls).toBe(0)
  await write(resolve(state, "seen/123-a2.json"), { retained: true })
  await run(configure(["probe", "123", "2"], directory, api, send))
  expect(calls).toBe(2)
  expect(ids.size).toBe(2)
  expect(JSON.parse(await fs.readFile(resolve(state, "seen/123-a2.json"), "utf8"))).toEqual({ retained: true })
})
