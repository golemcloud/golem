import { afterEach, beforeEach, expect, test } from "bun:test"
import { Effect } from "effect"
import * as fs from "node:fs/promises"
import { resolve } from "node:path"
import { serve } from "./dashboard/server.ts"
import { changesSincePreviousRun, preparePublication, publish } from "./publish-dashboard.ts"
import { array, checkedCommand, io, object } from "./runtime.ts"
import { observation, run, scratch, summary, write } from "./test-helpers.ts"

let directory: string, state: string, base: string, first: string, second: string
const git = (...args: string[]) =>
  run(
    checkedCommand(["git", ...args], {
      cwd: directory,
      env: {
        ...process.env,
        GIT_AUTHOR_NAME: "Fixture",
        GIT_AUTHOR_EMAIL: "fixture@example.invalid",
        GIT_COMMITTER_NAME: "Fixture",
        GIT_COMMITTER_EMAIL: "fixture@example.invalid"
      }
    })
  )
beforeEach(async () => {
  directory = await scratch("changes")
  state = resolve(directory, "state")
  await fs.mkdir(resolve(state, "seen"), { recursive: true })
  await git("init", "-q")
  await git("commit", "--allow-empty", "-qm", "Initial source")
  base = (await git("rev-parse", "HEAD")).trim()
  await git("commit", "--allow-empty", "-qm", "First change <safe> & literal")
  first = (await git("rev-parse", "HEAD")).trim()
  await git("commit", "--allow-empty", "-qm", "Second change")
  second = (await git("rev-parse", "HEAD")).trim()
})
afterEach(async () => {
  await fs.rm(directory, { recursive: true, force: true })
})
const facts = () => ({ ...observation(), run_id: 50, source: second, created_at: "2026-11-18T07:00:00Z" })
const seen = (id: number, source: string, time: string, fields = {}) =>
  write(resolve(state, "seen", `${id}.json`), {
    run_id: id,
    attempt: 1,
    source,
    selected: true,
    context: "Main validation",
    classification: "green",
    conclusion: "success",
    created_at: `2026-11-18T${time}:00Z`,
    ...fields
  })
test("complete range is oldest-first with literal subjects; same source is verified empty", async () => {
  await seen(40, base, "06:00")
  const result = await run(changesSincePreviousRun(facts(), state, directory))
  expect(result.available).toBe(true)
  expect(result.baseline).toBe(base)
  expect(result.commits).toEqual([{ sha: first, subject: "First change <safe> & literal" }, {
    sha: second,
    subject: "Second change"
  }])
  await seen(40, second, "06:00")
  expect(await run(changesSincePreviousRun(facts(), state, directory))).toMatchObject({ available: true, commits: [] })
})
test("previous failure, not last green; exclude context boundaries, cancelled, unselected, same run and later runs", async () => {
  await seen(30, base, "05:00")
  await seen(40, first, "06:00", { classification: "failure", conclusion: "failure" })
  await seen(41, second, "06:01", { context: "Main validation (acceptance)" })
  await seen(42, second, "06:02", { selected: false })
  await seen(43, second, "06:03", { conclusion: "cancelled" })
  await seen(50, second, "06:04")
  await seen(51, second, "08:00")
  expect(await run(changesSincePreviousRun(facts(), state, directory))).toMatchObject({
    baseline: first,
    commits: [{ sha: second, subject: "Second change" }]
  })
})
test("missing baseline, unknown identity, invalid history and shallow checkout are unavailable, not empty ranges", async () => {
  expect((await run(changesSincePreviousRun(facts(), state, directory))).available).toBe(false)
  await seen(40, base, "06:00")
  for (const fields of [{ source: "" }, { context: "" }]) {
    expect(await run(changesSincePreviousRun({ ...facts(), ...fields }, state, directory))).toMatchObject({
      available: false,
      baseline: "",
      commits: [],
      note: "The tested source and context are not yet verified."
    })
  }
  await seen(40, "a".repeat(40), "06:00")
  expect((await run(changesSincePreviousRun(facts(), state, directory))).available).toBe(false)
  await seen(40, base, "06:00")
  await fs.writeFile(resolve(directory, ".git/shallow"), base + "\n")
  expect((await run(changesSincePreviousRun(facts(), state, directory))).note).toContain(
    "Full Git history is unavailable"
  )
})
test("divergent history enumerates reachable commits and explains divergence", async () => {
  await git("checkout", "-q", "--detach", base)
  await git("commit", "--allow-empty", "-qm", "Other branch")
  await seen(40, (await git("rev-parse", "HEAD")).trim(), "06:00")
  const result = await run(changesSincePreviousRun(facts(), state, directory))
  expect(result.available).toBe(true)
  expect(result.note).toContain("diverge")
  expect(result.commits.map((c) => c.sha)).toEqual([first, second])
})
test("publisher preserves multiline summary through real HTTP, rejects credentials before sending, never consults public progress for identity", async () => {
  await seen(40, base, "06:00")
  const server = await run(serve(resolve(directory, "dashboard"), "fixture-token", "", 0, "127.0.0.1"))
  let sends = 0
  const send = (_url: string, init?: RequestInit) =>
    io(async () => {
      sends++
      const response = await fetch(`${server.url}api/run`, init)
      return { status: response.status, body: await response.text() }
    })
  const env = { CI_DASHBOARD_URL: "https://dashboard.example.invalid", CI_DASHBOARD_TOKEN: "fixture-token" }
  try {
    const payload = await run(
      preparePublication(facts(), { classification: "failure", analysis: { summary } }, state, directory)
    )
    expect(await run(publish(payload, "/api/run", env, send))).toBe("https://dashboard.example.invalid/#run-50-a1")
    const saved = object(array(object(await fetch(`${server.url}data.json`).then((r) => r.json())).runs)[0])
    expect(object(saved.analysis).summary).toBe(summary)
    expect(object(saved.analysis).published_at).not.toBe(facts().updated_at)
    for (const fields of [{ source: "" }, { context: "" }, { source: "", context: "" }]) {
      const value = await run(
        preparePublication(
          { ...facts(), ...fields },
          { classification: "running", analysis: { summary } },
          state,
          directory
        )
      )
      expect(object(value.changes).available).toBe(false)
      expect(object(value.changes).baseline).toBe("")
      expect(object(value.analysis).published_at).toBeUndefined()
    }
    const before = sends
    await expect(run(publish({ ...payload, analysis: { summary: env.CI_DASHBOARD_TOKEN } }, "/api/run", env, send)))
      .rejects.toThrow("credential")
    expect(sends).toBe(before)
    for (
      const url of [
        "http://insecure.invalid",
        "https://user:pass@example.invalid",
        "https://example.invalid/?token=secret",
        "https://example.invalid/#fragment"
      ]
    ) await expect(run(publish({}, "/api/run", { ...env, CI_DASHBOARD_URL: url }, send))).rejects.toThrow()
  } finally {
    await server.stop(true)
  }
})
test("invalid publication boundaries fail normally and never retry an unconfirmed write", async () => {
  expect((await run(Effect.result(preparePublication([], {}))))._tag).toBe("Failure")
  const env = { CI_DASHBOARD_URL: "https://dashboard.example.invalid", CI_DASHBOARD_TOKEN: "fixture-token" }
  for (const body of ["not JSON", "null", "{\"status\":\"pending\"}"]) {
    let sends = 0
    const result = await run(Effect.result(publish({}, "/api/heartbeat", env, () => {
      sends++
      return Effect.succeed({ status: 200, body })
    })))
    expect(result._tag).toBe("Failure")
    expect(sends).toBe(1)
  }
  await fs.writeFile(resolve(state, "seen/broken.json"), "not JSON")
  expect(await run(changesSincePreviousRun(facts(), state, directory))).toMatchObject({ available: false, commits: [] })
})
