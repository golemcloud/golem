import { FileSystem } from "@effect/platform"
import { Console, Effect, Either, Schema } from "effect"
import { randomUUID } from "node:crypto"
import { availableParallelism } from "node:os"
import { join } from "node:path"
import { parseArgs } from "node:util"
import features from "./features.json" with { type: "json" }
import {
  baselineChanges,
  type Case,
  Corpus,
  credited,
  decodeBytes,
  differingField,
  encodeBytes,
  expectedFor,
  featureTable,
  Golden,
  goldenFor,
  hash,
  type Outcome,
  redundantFixture,
  splitCalls,
  ValidationError,
  wasmStatus
} from "./model.ts"
import { absolute, attempt, here, main, readJson, run, runChecked, temporary, writeJson } from "./platform.ts"

const golemEnv = {
  GOLEM_AGENT_ID: "conformance-agent",
  GOLEM_WORKER_NAME: "conformance-agent",
  GOLEM_COMPONENT_ID: "00000000-0000-0000-0000-000000000000",
  GOLEM_COMPONENT_REVISION: "0",
  GOLEM_AGENT_TYPE: "Conformance"
}
const oracleEnv = { LC_ALL: "C.UTF-8", PATH: "/usr/local/bin:/usr/bin:/bin" }
const oracleUser = "1000:1000"
const wasmtimeOptions = [
  "-Sp3",
  "-Shttp",
  "-Wcomponent-model-async=y",
  "-Ccache=n",
  "-Wmax-wasm-stack=1048576",
  "-Wasync-stack-size=2621440"
]
const dockerfile = join(here, "oracle.Dockerfile")
const CommandName = Schema.Literal("check", "record", "live", "stale", "coverage")
export function argumentsFor(argv: string[]) {
  const { values: v, positionals } = parseArguments(argv)
  const command = positionals[0] ?? "check"
  if (
    positionals.length > 1 ||
    !Schema.is(CommandName)(command)
  ) {
    throw new ValidationError({ message: "usage: conformance.ts [check|record|live|stale|coverage] [options]" })
  }
  if (!["pr", "all"].includes(v.tier)) {
    throw new ValidationError({ message: "--tier must be pr or all" })
  }
  const jobs = Number(v.jobs)
  const runs = Number(v.runs)
  if (
    !Number.isInteger(jobs) ||
    jobs < 1 ||
    !Number.isInteger(runs) ||
    runs < 1
  ) {
    throw new ValidationError({ message: "--jobs and --runs must be positive integers" })
  }
  if (v["update-known-failures"] && !v["known-failures"]) {
    throw new ValidationError({ message: "--update-known-failures needs --known-failures" })
  }
  if (command === "check" || command === "live" || command === "coverage") {
    if (!v.wasm) throw new ValidationError({ message: `${command} needs --wasm` })
    return { ...v, jobs, runs, command, wasm: v.wasm }
  }
  return { ...v, jobs, runs, command }
}
function parseArguments(argv: string[]) {
  try {
    return parseArgs({
      args: argv,
      allowPositionals: true,
      options: {
        wasm: { type: "string" },
        wasmtime: { type: "string", default: "wasmtime" },
        "oracle-image": {
          type: "string",
          default: "golem-cooperative-bash5-oracle:local"
        },
        case: { type: "string", multiple: true, default: [] },
        prefix: { type: "string", multiple: true, default: [] },
        jobs: {
          type: "string",
          default: process.env.BASH_CONFORMANCE_JOBS ?? String(availableParallelism())
        },
        report: { type: "string" },
        tier: { type: "string", default: "pr" },
        "known-failures": { type: "string" },
        "update-known-failures": { type: "boolean", default: false },
        runs: { type: "string", default: "2" }
      }
    })
  } catch (error) {
    if (error instanceof TypeError && "code" in error && String(error.code).startsWith("ERR_PARSE_ARGS_")) {
      throw new ValidationError({ message: error.message })
    }
    throw error
  }
}
type Args = ReturnType<typeof argumentsFor>
type WasmArgs = Extract<Args, { wasm: string }>
export const loadCases = Effect.fn("conformance.loadCases")(function*(directory = join(here, "cases")) {
  const fs = yield* FileSystem.FileSystem
  const names = new Set<string>()
  const cases: Case[] = []
  for (
    const file of (yield* fs.readDirectory(directory))
      .filter((n) => n.endsWith(".json"))
      .sort()
  ) {
    const corpus = yield* readJson(join(directory, file), Corpus)
    for (const c of corpus.cases) {
      if (names.has(c.name)) {
        return yield* Effect.fail(
          new ValidationError({ message: `duplicate case name ${c.name} in ${file}` })
        )
      }
      names.add(c.name)
      cases.push({ ...c, corpus: file.slice(0, -5), tier: corpus.tier })
    }
  }
  if (!cases.length) {
    return yield* Effect.fail(new ValidationError({ message: `no cases in ${directory}` }))
  }
  return cases
})

// Match Python's json.dumps(sort_keys=True) bytes used by the recorded profile fingerprint.
type ProfileValue = string | { readonly [key: string]: ProfileValue }
function profileJson(value: ProfileValue): string {
  if (typeof value === "object") {
    return (
      "{" +
      Object.entries(value)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([k, v]) => JSON.stringify(k) + ": " + profileJson(v))
        .join(", ") +
      "}"
    )
  }
  return JSON.stringify(value)
}
export const oracleFingerprint = Effect.gen(function*() {
  const fs = yield* FileSystem.FileSystem
  return {
    dockerfile: hash(yield* fs.readFile(dockerfile)),
    profile: hash(
      profileJson({
        env: golemEnv,
        oracle_env: oracleEnv,
        user: oracleUser,
        // Change the setup version when filesystem setup changes oracle answers.
        setup: "writable-root-v2"
      })
    )
  }
})
const readGolden = Effect.fn("conformance.readGolden")(function*(corpus: string) {
  const fs = yield* FileSystem.FileSystem
  const path = join(here, "goldens", `${corpus}.json`)
  return (yield* fs.exists(path))
    ? yield* readJson(path, Golden)
    : ({ oracle: null, cases: new Map() } satisfies Golden)
})
const loadGoldenCorpora = (cases: readonly Case[]) =>
  Effect.forEach(
    [...new Set(cases.map((c) => c.corpus))],
    (corpus) =>
      readGolden(corpus).pipe(
        Effect.map((golden) => ({ corpus, golden, cases: cases.filter((c) => c.corpus === corpus) }))
      )
  )
const compile = (args: WasmArgs, base: string) =>
  runChecked(
    [
      args.wasmtime,
      "compile",
      ...wasmtimeOptions,
      "-o",
      join(base, "shell.cwasm"),
      absolute(args.wasm)
    ],
    120
  ).pipe(Effect.as(join(base, "shell.cwasm")))
export const runOurs = (c: Case, compiled: string, wasmtime: string) =>
  Effect.scoped(
    Effect.gen(function*() {
      const fs = yield* FileSystem.FileSystem
      const root = yield* temporary()
      const report = yield* temporary()
      if (!c.options.no_tmp) yield* fs.makeDirectory(join(root, "tmp"))
      const command: [string, ...string[]] = [
        wasmtime,
        "run",
        ...wasmtimeOptions,
        "--allow-precompiled",
        "--dir",
        `${root}::/`,
        "--dir",
        `${report}::/.conformance`
      ]
      for (const [key, value] of Object.entries(golemEnv)) {
        command.push("--env", `${key}=${value}`)
      }
      command.push(compiled, "--status-file", "/.conformance/status")
      const calls = splitCalls(c.script)
      command.push(
        ...(calls.length > 1
          ? ["--calls", ...calls.map((call) => call.script)]
          : [c.script])
      )
      const result = yield* run(command, c.options.timeout ?? 15)
      const status = yield* fs
        .readFileString(join(report, "status"))
        .pipe(
          Effect.mapError(
            (error) =>
              new ValidationError({
                message: `missing shell status report: ${error}; stderr=${JSON.stringify(result.stderr)}`
              })
          )
        )
      return {
        ...result,
        status: yield* attempt(() => wasmStatus(result.status, status)).pipe(
          Effect.mapError((error) =>
            new ValidationError({
              message: `${error.message}; stderr=${JSON.stringify(result.stderr)}`
            })
          )
        )
      }
    })
  )

const buildOracle = (image: string) =>
  runChecked(
    ["docker", "build", "-q", "-f", dockerfile, "-t", image, here],
    600
  )
const oracle = Effect.fn("conformance.oracle")(function*(image: string) {
  const name = `golem-conformance-${randomUUID().slice(0, 12)}`
  yield* Effect.addFinalizer(() =>
    runChecked(["docker", "rm", "-f", name], 60).pipe(
      Effect.catchAll((error) => Console.error(`oracle cleanup ${name}: ${error}`))
    )
  )
  yield* runChecked(
    [
      "docker",
      "run",
      "-d",
      "--rm",
      "--name",
      name,
      "--user",
      oracleUser,
      "--network",
      "none",
      "--workdir",
      "/",
      "--entrypoint",
      "sleep",
      image,
      "infinity"
    ],
    120
  )
  const root = (script: string) => runChecked(["docker", "exec", "-u", "0", name, "sh", "-c", script])
  const initial = yield* root("chmod 1777 / && ls -A /")
  const baseline = decodeBytes(initial.stdout)
    .toString()
    .split(/\s+/)
    .filter((n) => n && n !== "tmp")
    .join(" ")
  return (c: Case) =>
    Effect.gen(function*() {
      yield* root(
        `kill -9 -1 2>/dev/null; cd / || exit 1; for entry in $(ls -A /); do case " ${baseline} " in *" $entry "*) ;; *) rm -rf "/$entry" ;; esac; done; ` +
          (c.options.no_tmp ? "true" : "mkdir -m 1777 /tmp")
      )
      let status = 0
      const stdout: Buffer[] = []
      const stderr: Buffer[] = []
      for (const call of splitCalls(c.script)) {
        const result = yield* run(
          [
            "docker",
            "exec",
            "-u",
            oracleUser,
            "-w",
            call.cwd,
            name,
            "env",
            "-i",
            ...Object.entries({ ...oracleEnv, ...golemEnv }).map(
              ([k, v]) => `${k}=${v}`
            ),
            "bash",
            "-c",
            call.script
          ],
          c.options.timeout ?? 15
        )
        status = result.status
        stdout.push(decodeBytes(result.stdout))
        stderr.push(decodeBytes(result.stderr))
      }
      return {
        status,
        stdout: encodeBytes(Buffer.concat(stdout)),
        stderr: encodeBytes(Buffer.concat(stderr))
      }
    })
})
const runOracle = Effect.fn("conformance.runOracle")(function*(cases: readonly Case[], args: Args) {
  const workers = Math.min(args.jobs, cases.length)
  // Each worker owns one container. Every record pass creates a new set of containers.
  const results = yield* Effect.forEach(
    Array.from({ length: workers }, (_, i) => i),
    (i) =>
      Effect.scoped(
        Effect.gen(function*() {
          const execute = yield* oracle(args["oracle-image"])
          const results: [string, Either.Either<Outcome, string>][] = []
          for (const c of cases.filter((_, n) => n % workers === i)) {
            const result = yield* execute(c).pipe(
              Effect.mapError(String),
              Effect.either
            )
            results.push([c.name, result])
          }
          return results
        })
      ),
    { concurrency: "unbounded" }
  )
  return new Map(results.flat())
})

const checkBaseline = Effect.fn("conformance.checkBaseline")(function*(
  args: Args,
  cases: readonly Case[],
  failures: string[]
) {
  const path = args["known-failures"]
  if (!path) return failures.length === 0
  if (args["update-known-failures"]) {
    yield* writeJson(path, failures.sort())
    return true
  }
  const fs = yield* FileSystem.FileSystem
  const known = new Set(
    (yield* fs.exists(path))
      ? yield* readJson(path, Schema.Array(Schema.String))
      : []
  )
  const changes = baselineChanges(
    known,
    new Set(cases.map((c) => c.name)),
    new Set(failures)
  )
  for (const name of changes.new) yield* Console.log(`NEW FAILURE ${name}`)
  for (const name of changes.fixed) {
    yield* Console.log(`NOW PASSES ${name} (remove it from ${path})`)
  }
  yield* Console.log(
    `${failures.length} failing; ${changes.new.length} new, ${changes.fixed.length} fixed`
  )
  return !changes.new.length && !changes.fixed.length
})

const compare = (
  args: WasmArgs,
  cases: readonly Case[],
  expectation: (c: Case) => Outcome
) =>
  Effect.scoped(
    Effect.gen(function*() {
      const base = yield* temporary()
      const compiled = yield* compile(args, base)
      const records = yield* Effect.forEach(
        cases,
        (c) =>
          Effect.gen(function*() {
            const expectedResult = yield* attempt(() => expectation(c)).pipe(
              Effect.either
            )
            const result = Either.isLeft(expectedResult)
              ? expectedResult
              : yield* runOurs(c, compiled, args.wasmtime).pipe(Effect.either)
            const expected = Either.getOrNull(expectedResult)
            const actual = Either.getOrNull(result)
            const error = Either.isLeft(result) ? String(result.left) : null
            const failed = error || (expected && actual && differingField(expected, actual))
            yield* Console.log(
              `${failed ? "FAIL" : "PASS"} ${c.name}${
                failed ? `: ${error ?? JSON.stringify({ expected, actual })}` : ""
              }`
            )
            return failed
              ? {
                name: c.name,
                corpus: c.corpus,
                tags: c.tags,
                script: c.script,
                expected,
                actual,
                error
              }
              : null
          }),
        { concurrency: args.jobs }
      )
      const failures = records
        .filter((r) => r !== null)
        .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
      yield* Console.log(
        `${failures.length} failures of ${cases.length} cases`
      )
      if (args.report) yield* writeJson(args.report, failures)
      return yield* checkBaseline(
        args,
        cases,
        failures.map((r) => r.name)
      )
    })
  )

const coverage = (args: WasmArgs, cases: readonly Case[]) =>
  Effect.scoped(
    Effect.gen(function*() {
      const counts = new Map(Object.keys(featureTable).map((tag) => [tag, 0]))
      const problems: string[] = []
      for (const c of cases) {
        const { found, unknown } = credited(c)
        for (const tag of unknown) {
          problems.push(`${c.name}: unknown tag ${tag}`)
        }
        for (const tag of found) counts.set(tag, (counts.get(tag) ?? 0) + 1)
      }
      const base = yield* temporary()
      const compiled = yield* compile(args, base)
      const result = yield* runOurs(
        {
          name: "compgen",
          script: "compgen -b; echo --; compgen -c; echo --; compgen -k",
          corpus: "coverage",
          tier: "pr",
          tags: [],
          options: {}
        },
        compiled,
        args.wasmtime
      )
      if (result.status) {
        return yield* Effect.fail(
          new ValidationError({ message: `compgen failed: ${JSON.stringify(result.stderr)}` })
        )
      }
      const parts = decodeBytes(result.stdout).toString().split("--\n")
      const [builtinNames, allNames, keywordNames] = yield* Schema.decodeUnknown(
        Schema.Tuple(Schema.String, Schema.String, Schema.String)
      )(parts)
      const words = (text: string) => new Set(text.split(/\s+/).filter(Boolean))
      const builtins = words(builtinNames)
      const all = words(allNames)
      const keywords = words(keywordNames)
      const commands = new Set(
        [...all].filter((n) => !builtins.has(n) && !keywords.has(n))
      )
      for (
        const [registered, listed, kind] of [
          [
            builtins,
            new Set([...features.builtins, ...features.builtinReplacements]),
            "builtin"
          ],
          [
            commands,
            new Set([
              ...features.commands.filter(
                (n) => !features.builtinReplacements.includes(n)
              ),
              ...features.fixtures
            ]),
            "command"
          ]
        ] as const
      ) {
        for (const n of registered) {
          if (!listed.has(n)) {
            problems.push(
              `${kind} ${n} is registered but not in features.json`
            )
          }
        }
        for (const n of listed) {
          if (!registered.has(n)) {
            problems.push(
              `${kind} ${n} is in features.json but not registered`
            )
          }
        }
      }
      const gaps = [...counts].filter(
        ([tag, count]) => !count && !(tag in features.notApplicable)
      )
      for (const [tag, count] of [...counts].sort()) {
        yield* Console.log(
          `${tag}  ${tag in features.notApplicable ? "n/a" : count}`
        )
      }
      for (const problem of problems) yield* Console.log(`PROBLEM ${problem}`)
      for (const [tag] of gaps) {
        yield* Console.log(`GAP ${tag} - ${featureTable[tag]}`)
      }
      yield* Console.log(`${gaps.length} gaps; ${problems.length} problems`)
      return !gaps.length && !problems.length
    })
  )

export const program = Effect.fn("conformance.program")(function*(argv: string[]) {
  const args = yield* attempt(() => argumentsFor(argv))
  const all = yield* loadCases()
  const selected = all.filter(
    (c) =>
      ((!args.case.length && !args.prefix.length) ||
        args.case.includes(c.name) ||
        args.prefix.some((p) => c.name.startsWith(p))) &&
      (args.command !== "check" || args.tier === "all" || c.tier === "pr")
  )
  if (!selected.length) {
    return yield* Effect.fail(new ValidationError({ message: "no cases selected" }))
  }
  if (args.command === "coverage") return yield* coverage(args, selected)
  if (args.command === "check") {
    const fingerprint = yield* oracleFingerprint
    const corpora = yield* loadGoldenCorpora(selected)
    const goldens = new Map(corpora.map(({ corpus, golden }) => [corpus, golden]))
    return yield* compare(args, selected, (c) => {
      const data = goldens.get(c.corpus)
      if (data === undefined) throw new Error(`missing golden group for ${c.corpus}`)
      const outcome = goldenFor(c, data, fingerprint)
      const redundant = redundantFixture(c, outcome)
      if (redundant) throw new ValidationError({ message: redundant })
      return expectedFor(c, outcome)
    })
  }
  yield* buildOracle(args["oracle-image"])
  if (args.command === "live") {
    const results = yield* runOracle(
      selected.filter((c) => !c.expected),
      args
    )
    return yield* compare(args, selected, (c) => {
      if (c.expected) return expectedFor(c, c.expected)
      const result = results.get(c.name)
      if (!result || Either.isLeft(result)) {
        throw new ValidationError({ message: result ? result.left : "oracle did not run" })
      }
      return expectedFor(c, result.right)
    })
  }
  const fingerprint = yield* oracleFingerprint
  const corpora = yield* loadGoldenCorpora(selected)
  if (args.command === "stale") {
    const needed = selected.filter((c) => !c.expected && !c.oracleRace)
    const results = yield* runOracle(needed, args)
    let stale = 0
    for (const { golden: data, cases } of corpora) {
      for (const c of cases) {
        if (c.expected || c.oracleRace) continue
        const result = results.get(c.name)
        const comparison = yield* attempt(() => {
          const golden = goldenFor(c, data, fingerprint)
          if (!result || Either.isLeft(result)) {
            throw new ValidationError({ message: result ? result.left : "oracle did not run" })
          }
          const field = differingField(golden, result.right)
          if (field) {
            throw new ValidationError({ message: `oracle now answers ${JSON.stringify(result.right)} (${field})` })
          }
        }).pipe(Effect.either)
        if (comparison._tag === "Left") {
          stale++
          yield* Console.log(`STALE ${c.name}: ${comparison.left}`)
        }
      }
    }
    yield* Console.log(`${stale} stale of ${needed.length} goldens`)
    return stale === 0
  }
  const runs = yield* Effect.forEach(Array.from({ length: args.runs }), () => runOracle(selected, args))
  let failures = 0
  let recorded = 0
  const names = new Set(all.map((c) => c.name))
  for (const { corpus, golden: old, cases } of corpora) {
    const entries = new Map<string, Outcome & { script: string }>(
      old.oracle?.dockerfile === fingerprint.dockerfile &&
        old.oracle?.profile === fingerprint.profile
        ? old.cases
        : []
    )
    for (const c of cases) {
      const values = runs.map((run) => run.get(c.name))
      const outcomes = values.flatMap((v) => v && Either.isRight(v) ? [v.right] : [])
      const first = outcomes[0]
      if (!first || outcomes.length !== args.runs) {
        failures++
        yield* Console.log(`ERROR ${c.name}: ${JSON.stringify(values)}`)
        continue
      }
      const field = outcomes
        .slice(1)
        .map((o) => differingField(first, o))
        .find(Boolean)
      if (field) {
        failures++
        yield* Console.log(
          `NONDETERMINISTIC ${c.name}: ${field} differs between runs`
        )
        continue
      }
      entries.set(c.name, { ...first, script: hash(c.script) })
      recorded++
    }
    const data = {
      cases: Object.fromEntries(
        [...entries]
          .filter(([n]) => names.has(n))
          .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
          .map(([name, { script, status, stderr, stdout }]) => [name, { script, status, stderr, stdout }])
      ),
      oracle: fingerprint
    }
    yield* writeJson(join(here, "goldens", `${corpus}.json`), data)
  }
  yield* Console.log(`recorded ${recorded} of ${selected.length} cases`)
  return failures === 0
})
if (import.meta.main) main(program(process.argv.slice(2)))
