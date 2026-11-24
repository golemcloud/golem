import { test, expect } from "bun:test"
import { readdirSync, readFileSync } from "fs"
import { join, relative } from "path"

// 2.0 removed principal injection and the agent-wide principal getters. Code reads its caller
// from the invocation context (`ctx.caller`) and stores `ctx.caller.original.ref` as a
// `PrincipalRef`. These names and shapes exist only in 1.6, so a page, skill or template that
// shows one teaches code that no longer builds.
const removed: [string, RegExp][] = [
  ["agent-wide principal getter", /\bget_?[Pp]rincipal\b/],
  ["principal input marker", /\bs\.principal\(/],
  ["Effect principal schema", /\bPrincipalSchema\b/],
  ["Effect principal service", /\bPrincipal\.Principal\b/],
  ["principal field of a context", /\b(ctx|context|init|initContext|InitContext|ic)\.principal\b/],
  ["Go principal accessor", /\bPrincipal\(\)/],
  ["principal cache flag", /\buses[-_]?[Pp]rincipal\b/],
  ["principal parameter", /\b\w+\s*:\s*&?(golem\.|@\w+\.)?Principal\b(?!Ref)/],
  ["Go principal field", /^\s*\w+\s+golem\.Principal\b/m],
  ["injected principal", /\binjected\s+`?(golem\.|@\w+\.)?Principal\b/i],
  ["skipped principal field", /\b(skip|exclude|excluding)\s+`?(golem\.|@\w+\.)?Principal`?\s+fields?\b/i],
  ["per-principal cache", /\bper-principal\b/i],
  ["1.6 OIDC principal", /\bOidcPrincipal\b/],
]

// Pages that must name the removed API, such as a 1.6 to 2.0 migration guide, go here.
const allowed = new Set<string>([])

const repo = join(import.meta.dir, "../..")
const roots = ["docs/src/content/next", "golem-skills/skills", "cli/golem-cli/templates"]

function files(root: string): string[] {
  return readdirSync(join(repo, root), { recursive: true, encoding: "utf8" })
    .filter(path => /\.(md|mdx)$/.test(path))
    .map(path => join(root, path))
    .filter(path => !allowed.has(path))
}

test("docs, skills and templates teach no removed 1.6 principal API", () => {
  const hits: string[] = []
  for (const root of roots) {
    for (const path of files(root)) {
      readFileSync(join(repo, path), "utf8")
        .split("\n")
        .forEach((line, index) => {
          for (const [name, pattern] of removed) {
            if (pattern.test(line)) hits.push(`${path}:${index + 1}: ${name}`)
          }
        })
    }
  }
  expect(hits).toEqual([])
})

test("the removed-API patterns match the 1.6 shapes and spare the 2.0 ones", () => {
  const matches = (line: string) => removed.some(([, pattern]) => pattern.test(line))
  for (const line of [
    "whoAmI: method({ input: { caller: s.principal() } })",
    "const principal: Principal = this.getPrincipal();",
    "input: { caller: Principal.PrincipalSchema },",
    "const principal = yield* Principal.Principal",
    "fn my_visible_items(&self, principal: Principal) -> Vec<Item>;",
    "def adminOnly(principal: Principal): Future[AdminData]",
    "pub fn SecureAgent::who_am_i(self: Self, principal: Principal) -> String {",
    "init: (ctx) => ({ caller: ctx.principal })",
    "- `ctx.Parameters()`, `ctx.Principal()` and `ctx.ToolName()`",
    "\tCaller golem.Principal",
    "Skip host-injected `Principal` fields.",
    "There is no `uses_principal` option.",
    "Skip `golem.Principal` fields: the host fills them.",
    "## Per-principal caching",
  ]) {
    expect({ line, matches: matches(line) }).toEqual({ line, matches: true })
  }
  for (const line of [
    "whoAmI(_input, ctx) { return { kind: ctx.caller.original.kind } }",
    "creator: s.principalRef(),",
    "const ref: PrincipalRef = ctx.caller.original.ref",
    "fn who_am_i(&self, ctx: InvocationContext) -> String {",
    "def whoAmI(ctx: InvocationContext): Future[String]",
    "let creator = ctx.caller().original().principal_ref().clone();",
    "ref, ok := ctx.Caller().Original.Ref()",
    "store `caller.original.ref` as an `InvocationContext.PrincipalRefSchema` value",
    "A `Principal` or a `Caller` cannot be a method input or result.",
    "`original caller:` is the principal that started the chain of calls",
  ]) {
    expect({ line, matches: matches(line) }).toEqual({ line, matches: false })
  }
})
