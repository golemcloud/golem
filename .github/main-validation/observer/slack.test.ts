import { afterEach, beforeEach, expect, test } from "bun:test"
import { Effect } from "effect"
import * as fs from "node:fs/promises"
import { resolve } from "node:path"
import { postSlack } from "./post-slack.ts"
import { io, object } from "./runtime.ts"
import { run, scratch, write } from "./test-helpers.ts"
let directory: string, message: string
const text = "*Main validation failed (acceptance)*\n*Evidence*\n• `fixture::test`: `timeout`"
beforeEach(async () => {
  directory = await scratch("slack")
  message = resolve(directory, "message.txt")
  await fs.writeFile(message, text + "\n")
})
afterEach(async () => {
  await fs.rm(directory, { recursive: true, force: true })
})
const env = () => ({
  OBSERVER_STATE_DIR: directory,
  CI_DASHBOARD_URL: "https://dashboard.example.invalid/",
  SLACK_BENCHMARK_WEBHOOK_URL: "https://slack.example.invalid/fixture"
})
const receipt = () => resolve(directory, "receipts/123-a2.json")
test("mrkdwn and labeled attempt permalink preserve message; only exact success confirms", async () => {
  for (
    const [status, body, delivery] of [[200, "ok", "confirmed"], [200, "ok\n", "unknown"], [201, "ok", "unknown"], [
      400,
      "no",
      "rejected"
    ]] as const
  ) {
    const record = await run(postSlack("123", "2", "failure", message, env(), (_url, init) => {
      expect(JSON.parse(String(init?.body))).toEqual({
        text: `${text}\n\n<https://dashboard.example.invalid/#run-123-a2|Dashboard>`,
        mrkdwn: true
      })
      return Effect.succeed({ status, body })
    }))
    expect(record.delivery).toBe(delivery)
    expect(JSON.parse(await fs.readFile(receipt(), "utf8"))).toEqual(record)
    await fs.unlink(receipt())
  }
})
test("existing confirmed, pending, rejected and unknown receipts are immutable and never post", async () => {
  await fs.mkdir(resolve(directory, "receipts"))
  for (const delivery of ["confirmed", "pending", "unknown", "rejected"]) {
    await write(receipt(), { delivery })
    const original = await fs.readFile(receipt(), "utf8")
    await expect(run(postSlack("123", "2", "failure", message, env(), () => {
      throw new Error("must not post")
    }))).rejects.toThrow("not sending again")
    expect(await fs.readFile(receipt(), "utf8")).toBe(original)
  }
})
test("connection failure retains unknown receipt; concurrent posts exclusively create one receipt", async () => {
  let calls = 0
  const send = () => {
    calls++
    return Effect.fail(Object.assign(new Error("fixture timeout"), { name: "TimeoutError" }))
  }
  const outcomes = await Promise.allSettled([
    run(postSlack("123", "2", "failure", message, env(), send)),
    run(postSlack("123", "2", "failure", message, env(), send))
  ])
  expect(calls).toBe(1)
  expect(outcomes.filter((o) => o.status === "fulfilled")).toHaveLength(1)
  const record = object(JSON.parse(await fs.readFile(receipt(), "utf8")))
  expect(record.delivery).toBe("unknown")
  expect(record.errorType).toBe("TimeoutError")
  await expect(run(postSlack("123", "2", "failure", message, env(), send))).rejects.toThrow("not sending again")
  expect(calls).toBe(1)
})
test("malformed existing receipts still block sends and typed I/O preserves delivery error names", async () => {
  await fs.mkdir(resolve(directory, "receipts"))
  for (const body of ["partial JSON", "[]"]) {
    await fs.writeFile(receipt(), body)
    let calls = 0
    const result = await run(Effect.result(postSlack("123", "2", "failure", message, env(), () => {
      calls++
      return Effect.succeed({ status: 200, body: "ok" })
    })))
    expect(result._tag).toBe("Failure")
    expect(calls).toBe(0)
    expect(await fs.readFile(receipt(), "utf8")).toBe(body)
  }
  await fs.unlink(receipt())
  const record = await run(postSlack("123", "2", "failure", message, env(), () =>
    io(async () => {
      throw new TypeError("fixture transport failure")
    })))
  expect(record).toMatchObject({ delivery: "unknown", errorType: "TypeError" })
})
test("real local HTTP Slack sink sees pending receipt before receiving request", async () => {
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch(request) {
      expect(object(JSON.parse(await fs.readFile(receipt(), "utf8"))).delivery).toBe("pending")
      expect(object(await request.json()).mrkdwn).toBe(true)
      return new Response("ok")
    }
  })
  try {
    expect(
      (await run(
        postSlack("123", "2", "failure", message, { ...env(), SLACK_BENCHMARK_WEBHOOK_URL: server.url.toString() })
      )).delivery
    ).toBe("confirmed")
  } finally {
    await server.stop(true)
  }
  const child = Bun.spawn(["bun", resolve(import.meta.dir, "post-slack.ts"), "123", "2", "failure", message], {
    env: { ...env(), PATH: process.env.PATH },
    stderr: "pipe",
    stdout: "pipe"
  })
  expect(await child.exited).toBe(1)
  expect(await new Response(child.stderr).text()).not.toContain(env().SLACK_BENCHMARK_WEBHOOK_URL)
})
