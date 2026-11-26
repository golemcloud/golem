import { expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join, resolve } from "node:path"

const root = resolve(import.meta.dirname, "../..")
const workflow = readFileSync(join(root, ".github/workflows/main-validation.yaml"), "utf8")
const release = readFileSync(join(root, ".github/workflows/prepare-golem-release.yaml"), "utf8")
const main = "a".repeat(40), frozen = "b".repeat(40)
function step(text: string, name: string): string {
  const value = text.split(`      - name: ${name}\n`)[1]
  if (!value) throw new Error(`missing step ${name}`)
  return value.split("\n      - ")[0]!
}
function script(text: string): string {
  const body = text.split("        run: |\n")[1]
  if (!body) throw new Error("missing literal shell")
  return body.split("\n").map((line) => line.replace(/^          /, "")).join("\n")
}
const publish = step(workflow, "Publish the exact-source result")
const workload = step(workflow, "Validate the selected source")
const success: Record<string, string> = {
  "needs.linux.result": "success",
  "needs.linux.outputs.workload_exit_code": "0",
  "needs.apfs.result": "success",
  "steps.linux-reports.outcome": "success",
  "steps.apfs-reports.outcome": "success",
  "steps.report.outcome": "success"
}
function passed(values: Record<string, string>): boolean {
  const expression = publish.match(/PASSED: \$\{\{ (.+) \}\}/)?.[1]
  if (!expression) throw new Error("missing result expression")
  const terms = expression.split(" && ").map((term) => {
    const match = term.match(/^([\w.-]+) == '([^']*)'$/)
    if (!match) throw new Error(`uncovered expression shape: ${term}`)
    return [match[1]!, match[2]!] as const
  })
  expect(Object.fromEntries(terms)).toEqual(success)
  expect(terms.length).toBe(Object.keys(success).length)
  return terms.every(([key, expected]) => (values[key] ?? "") === expected)
}
function fixture(check: (fixture: Fixture) => void) {
  const scratch = join(root, ".amp/in/orb-backup/workflow-tests")
  mkdirSync(scratch, { recursive: true })
  const directory = mkdtempSync(join(scratch, "case-"))
  try {
    check(new Fixture(directory))
  } finally {
    rmSync(directory, { recursive: true, force: true })
  }
}
class Fixture {
  readonly env: Record<string, string | undefined>
  readonly directory: string
  constructor(directory: string) {
    this.directory = directory
    const bin = join(directory, "bin")
    mkdirSync(bin)
    writeFileSync(
      join(bin, "gh"),
      `#!${process.execPath}\n
      import { appendFileSync, readFileSync } from "node:fs";
      const args = process.argv.slice(2);
      appendFileSync(process.env.CALLS, JSON.stringify(args) + "\\n");
      if (args.slice(0, 3).join(" ") === "api --method POST" || args.slice(0, 2).join(" ") === "issue close") process.exit(0);
      for (const [key, response] of Object.entries(JSON.parse(readFileSync(process.env.RESPONSES, "utf8")))) {
        if (!args.some(arg => arg.includes(key))) continue;
        if (response && typeof response === "object" && "exit" in response) {
          console.log(response.body ?? "API failure"); process.exit(response.exit);
        }
        console.log(typeof response === "string" ? response : JSON.stringify(response)); process.exit(0);
      }
      throw new Error("Unexpected gh call: " + JSON.stringify(args));
    `,
      { mode: 0o755 }
    )
    writeFileSync(
      join(bin, "git"),
      "#!/bin/sh\n[ \"$*\" = \"rev-parse HEAD\" ] || exit 1\nprintf \"%s\\n\" \"$TAG_SOURCE\"\n",
      { mode: 0o755 }
    )
    writeFileSync(
      join(bin, "bun"),
      "#!/bin/sh\nprintf \"%s\\n\" \"$*\" > \"$WORKLOAD_ARGS\"\nexit \"$WORKLOAD_STATUS\"\n",
      { mode: 0o755 }
    )
    this.env = {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      GITHUB_REPOSITORY: "fixture/repository",
      GITHUB_REF: "refs/heads/main",
      GITHUB_SHA: frozen,
      GITHUB_SERVER_URL: "https://github.invalid",
      GITHUB_RUN_ID: "123",
      RUNNER_TEMP: directory,
      GITHUB_OUTPUT: join(directory, "output"),
      GITHUB_STEP_SUMMARY: join(directory, "summary"),
      CALLS: join(directory, "calls"),
      RESPONSES: join(directory, "responses.json"),
      WORKLOAD_ARGS: join(directory, "workload-args"),
      ACCEPTANCE: "false",
      REQUESTED_SOURCE: "",
      TAG_SOURCE: frozen,
      SOURCE: main,
      REQUEST: "42",
      CONTEXT: "Main validation"
    }
  }
  read(name: string) {
    return readFileSync(join(this.directory, name), "utf8")
  }
  run(body: string, responses: Record<string, unknown> = {}, env: Record<string, string> = {}) {
    writeFileSync(this.env.RESPONSES!, JSON.stringify(responses))
    writeFileSync(this.env.CALLS!, "")
    rmSync(this.env.GITHUB_OUTPUT!, { force: true })
    return Bun.spawnSync(["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", body], {
      cwd: this.directory,
      env: { ...this.env, ...env },
      timeout: 10000
    })
  }
  select(responses: Record<string, unknown> = {}, env: Record<string, string> = {}) {
    return this.run(script(step(workflow.split("\n  linux:\n")[0]!, "Select an exact request or the latest main")), {
      "git/ref/heads/main": main,
      "issues?": [[]],
      "statuses?": [[]],
      ...responses
    }, env)
  }
  publication(values: Record<string, string>, expected: string) {
    const result = this.run(script(publish), {}, { PASSED: String(passed(values)) })
    expect(result.exitCode, result.stderr.toString()).toBe(expected === "success" ? 0 : 1)
    const calls: string[][] = this.read("calls").trim().split("\n").map((line) => JSON.parse(line))
    expect(calls.length).toBe(2)
    expect(calls[0]!.slice(0, 4)).toEqual(["api", "--method", "POST", `repos/fixture/repository/statuses/${main}`])
    expect(calls[0]).toContain(`state=${expected}`)
    expect(calls[0]).toContain("context=Main validation")
    expect(calls[1]!.slice(0, 5)).toEqual(["issue", "close", "42", "--repo", "fixture/repository"])
    expect(calls[1]!.join(" ")).toContain(`finished: ${expected}`)
    expect(this.read("summary")).toContain(`Validation of ${main}: ${expected}`)
  }
}
const status = (state = "success", context = "Main validation", creator = "github-actions[bot]") => ({
  state,
  context,
  creator: { login: creator }
})

test("workload shell retains real exit codes and output failures remain job failures", () =>
  fixture((f) => {
    for (const code of [0, 1, 17, 127, 143]) {
      const result = f.run(script(workload), {}, { WORKLOAD_STATUS: String(code) })
      expect(result.exitCode, result.stderr.toString()).toBe(0)
      expect(f.read("workload-args").trim()).toBe(
        ".validation-control/.github/main-validation/progress.ts observe -- cargo make --profile ci main-validation"
      )
      expect(f.read("output")).toBe(`exit_code=${code}\n`)
      f.publication(
        { ...success, "needs.linux.outputs.workload_exit_code": String(code) },
        code === 0 ? "success" : "failure"
      )
    }
    expect(f.run(script(workload), {}, { WORKLOAD_STATUS: "0", GITHUB_OUTPUT: "/dev/null" }).exitCode).toBe(0)
    expect(existsSync(join(f.directory, "output"))).toBe(false)
    const missing = { ...success }
    delete missing["needs.linux.outputs.workload_exit_code"]
    f.publication(missing, "failure")
    expect(
      f.run(script(workload), {}, { WORKLOAD_STATUS: "0", GITHUB_OUTPUT: join(f.directory, "missing/output") }).exitCode
    ).not.toBe(0)
    expect(existsSync(join(f.directory, "output"))).toBe(false)
    f.publication(
      { ...success, "needs.linux.result": "failure", "needs.linux.outputs.workload_exit_code": "" },
      "failure"
    )
  }))

test("all result conjunction combinations fail closed, and each failed prerequisite executes publication", () =>
  fixture((f) => {
    const keys = Object.keys(success)
    const choices = keys.map((key) =>
      key.endsWith("workload_exit_code")
        ? ["0", "1", "17", "", "success"]
        : ["success", "failure", "cancelled", "skipped", ""]
    )
    const visit = (index: number, values: Record<string, string>) => {
      if (index === keys.length) {
        expect(passed(values)).toBe(keys.every((key) => values[key] === success[key]))
        return
      }
      for (const value of choices[index]!) visit(index + 1, { ...values, [keys[index]!]: value })
    }
    visit(0, {})
    for (const [index, key] of keys.entries()) {
      for (const value of choices[index]!.slice(1)) f.publication({ ...success, [key]: value }, "failure")
    }
  }))

test("cache ownership and report delivery cannot mask infrastructure failures", () => {
  const linux = workflow.split("\n  linux:\n")[1]!.split("\n  apfs:\n")[0]!
  expect(linux).toContain("    outputs:\n      workload_exit_code: ${{ steps.workload.outputs.exit_code }}")
  for (const text of ["        id: workload\n", "        shell: bash\n", "        timeout-minutes: 300\n"]) {
    expect(workload).toContain(text)
  }
  expect(linux).toContain("    timeout-minutes: 330\n")
  expect(linux).not.toContain("continue-on-error")
  expect(linux).not.toContain("job-status:")
  expect(linux.split("uses: useblacksmith/stickydisk@v1").length - 1).toBe(2)
  for (
    const [name, key, path] of [["compiler", "target", "target"], ["dependency", "dependencies", ".validation-cache"]]
  ) {
    const cache = step(workflow, `Mount ${name} caches`)
    expect(cache).toContain(`          key: main-validation-linux-${key}\n`)
    expect(cache).toContain(`          path: ${path}\n`)
    expect(cache).toContain("          commit: 'true'")
  }
  const reports = step(workflow, "Retain Linux reports even after failure")
  for (
    const text of ["        if: always()\n", "          if-no-files-found: error\n", "          retention-days: 90"]
  ) expect(reports).toContain(text)
  expect(workflow).toContain("    needs: [select, linux, apfs]\n")
  expect(workflow).toContain("    if: always() && needs.select.outputs.selected == 'true'\n")
  expect(publish).toContain("        if: always()\n")
  expect(publish).not.toContain("continue-on-error")
})

test("main sampling skips only completed production statuses", () =>
  fixture((f) => {
    expect(workflow).not.toContain("MAIN_VALIDATION_ENABLED")
    expect(workflow).toContain("- cron: '*/10 * * * *'")
    expect(f.select().exitCode).toBe(0)
    expect(f.read("output")).toContain(`source=${main}\nrequest=\ncontext=Main validation\nselected=true`)
    expect(f.read("calls")).toContain("state=pending")
    for (const state of ["success", "failure", "error", "pending"]) {
      expect(f.select({ "statuses?": [[status(state)]] }).exitCode).toBe(0)
      expect(f.read("output").includes("selected=true")).toBe(state === "pending")
    }
    for (
      const entry of [
        status("success", "Main validation (acceptance)"),
        status("success", "Main validation", "someone-else")
      ]
    ) {
      expect(f.select({ "statuses?": [[entry]] }).exitCode).toBe(0)
      expect(f.read("output")).toContain("selected=true")
    }
  }))
test("acceptance remains separate; production rejects branch or source overrides", () =>
  fixture((f) => {
    for (const source of ["", main]) {
      expect(
        f.select({ "/commits/": "" }, {
          ACCEPTANCE: "true",
          GITHUB_REF: "refs/heads/feature",
          REQUESTED_SOURCE: source
        }).exitCode
      ).toBe(0)
      expect(f.read("output")).toContain(`source=${source || frozen}`)
      expect(f.read("output")).toContain("context=Main validation (acceptance)")
      expect(f.read("calls")).not.toContain("issues?")
    }
    const rejected: Record<string, string>[] = [{ GITHUB_REF: "refs/heads/feature" }, { REQUESTED_SOURCE: main }]
    for (const env of rejected) {
      expect(f.select({}, env).exitCode).not.toBe(0)
      expect(f.read("calls")).toBe("")
    }
  }))
test("frozen tag requests take priority without an ancestry requirement", () =>
  fixture((f) => {
    const issue = { number: 42, body: JSON.stringify({ source: frozen, release_tag: "v1.6.0" }) }
    for (const type of ["commit", "tag"]) {
      const result = f.select({
        "issues?": [[issue]],
        "git/matching-refs/": { type, sha: frozen },
        "git/tags/": { type: "commit", sha: frozen }
      })
      expect(result.exitCode, result.stderr.toString()).toBe(0)
      expect(f.read("output")).toContain(`source=${frozen}\nrequest=42`)
      expect(f.read("calls")).not.toContain("compare/")
      expect(f.read("calls")).not.toContain("\"issue\",\"close\"")
    }
  }))
test("invalid requests close before sampling; API failures stop selection", () =>
  fixture((f) => {
    expect(f.select({ "issues?": [[{ number: 42, body: "not JSON" }]] }).exitCode).toBe(0)
    expect(f.read("calls")).toContain("\"issue\",\"close\",\"42\"")
    expect(f.read("output")).toContain(`source=${main}`)
    const issue = { number: 42, body: JSON.stringify({ source: frozen }) }
    expect(f.select({ "issues?": [[issue]], "compare/": { exit: 1, body: "{\"status\":500}" } }).exitCode).not.toBe(0)
    expect(f.read("calls")).not.toContain("\"issue\",\"close\"")
    expect(f.read("output")).not.toContain("selected=true")
  }))
test("release gate needs latest exact-source bot production success", () =>
  fixture((f) => {
    const gate = step(
      release.split("\n  release-prerequisites:\n")[0]!,
      "Require completed validation of this exact release source"
    )
    expect(gate).not.toContain("        if:")
    expect(release).not.toContain("MAIN_VALIDATION_RELEASE_GATE_ENABLED")
    const cases: [unknown, boolean][] = [
      [[[status()]], true],
      [[[]], false],
      [[[status("pending"), status()]], false],
      [[[status("failure"), status()]], false],
      [[[status("error")]], false],
      [[[status("success", "Main validation (acceptance)")]], false],
      [[[status("success", "Main validation", "someone-else")]], false],
      [[[status("success", "Main validation (acceptance)"), status()]], true],
      [{ exit: 1 }, false],
      [{ exit: 0, body: "not json" }, false]
    ]
    for (const [response, accepted] of cases) {
      const result = f.run(script(gate), { "statuses?": response })
      expect(result.exitCode === 0, result.stderr.toString()).toBe(accepted)
      expect(f.read("calls")).toContain(`commits/${frozen}/statuses?`)
      expect(f.read("calls")).not.toContain("dispatch")
    }
  }))
