import { FileSystem } from "@effect/platform";
import { Console, Effect, Schema } from "effect";
import { availableParallelism } from "node:os";
import { join } from "node:path";
import { parseArgs } from "node:util";
import { randomUUID } from "node:crypto";
import {
  absolute,
  attempt,
  here,
  main,
  readJson,
  run,
  temporary,
  writeJson,
} from "./platform.ts";
import {
  baselineChanges,
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
  redundantFixture,
  splitCalls,
  wasmStatus,
  type Case,
  type Outcome,
} from "./model.ts";
import features from "./features.json";

const golemEnv = {
  GOLEM_AGENT_ID: "conformance-agent",
  GOLEM_WORKER_NAME: "conformance-agent",
  GOLEM_COMPONENT_ID: "00000000-0000-0000-0000-000000000000",
  GOLEM_COMPONENT_REVISION: "0",
  GOLEM_AGENT_TYPE: "Conformance",
};
const oracleEnv = { LC_ALL: "C.UTF-8", PATH: "/usr/local/bin:/usr/bin:/bin" };
const oracleUser = "1000:1000";
const wasmtimeOptions = [
  "-Sp3",
  "-Shttp",
  "-Wcomponent-model-async=y",
  "-Ccache=n",
  "-Wmax-wasm-stack=1048576",
  "-Wasync-stack-size=2621440",
];
const dockerfile = join(here, "oracle.Dockerfile");
export function argumentsFor(argv: string[]) {
  const { values: v, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: {
      wasm: { type: "string" },
      wasmtime: { type: "string", default: "wasmtime" },
      "oracle-image": {
        type: "string",
        default: "golem-cooperative-bash5-oracle:local",
      },
      case: { type: "string", multiple: true, default: [] },
      prefix: { type: "string", multiple: true, default: [] },
      jobs: {
        type: "string",
        default:
          process.env.BASH_CONFORMANCE_JOBS ?? String(availableParallelism()),
      },
      report: { type: "string" },
      tier: { type: "string", default: "pr" },
      "known-failures": { type: "string" },
      "update-known-failures": { type: "boolean", default: false },
      runs: { type: "string", default: "2" },
    },
  });
  const command = positionals[0] ?? "check";
  if (
    positionals.length > 1 ||
    !["check", "record", "live", "stale", "coverage"].includes(command)
  )
    throw new Error(
      "usage: conformance.ts [check|record|live|stale|coverage] [options]",
    );
  if (!["pr", "all"].includes(v.tier))
    throw new Error("--tier must be pr or all");
  const jobs = Number(v.jobs),
    runs = Number(v.runs);
  if (
    !Number.isInteger(jobs) ||
    jobs < 1 ||
    !Number.isInteger(runs) ||
    runs < 1
  )
    throw new Error("--jobs and --runs must be positive integers");
  if (["check", "live", "coverage"].includes(command) && !v.wasm)
    throw new Error(`${command} needs --wasm`);
  if (v["update-known-failures"] && !v["known-failures"])
    throw new Error("--update-known-failures needs --known-failures");
  return { ...v, jobs, runs, command };
}
type Args = ReturnType<typeof argumentsFor>;
export const loadCases = (directory = join(here, "cases")) =>
  Effect.gen(function* () {
    const fs = yield* FileSystem.FileSystem;
    const names = new Set<string>();
    const cases: Case[] = [];
    for (const file of (yield* fs.readDirectory(directory))
      .filter((n) => n.endsWith(".json"))
      .sort()) {
      const corpus = yield* readJson(join(directory, file), Corpus);
      for (const c of corpus.cases) {
        if (names.has(c.name))
          return yield* Effect.fail(
            new Error(`duplicate case name ${c.name} in ${file}`),
          );
        names.add(c.name);
        cases.push({ ...c, corpus: file.slice(0, -5), tier: corpus.tier });
      }
    }
    if (!cases.length)
      return yield* Effect.fail(new Error(`no cases in ${directory}`));
    return cases;
  });

// Keep the oracle environment fingerprint independent of object insertion order.
function profileJson(value: unknown): string {
  if (value !== null && typeof value === "object")
    return (
      "{" +
      Object.entries(value)
        .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
        .map(([k, v]) => JSON.stringify(k) + ": " + profileJson(v))
        .join(", ") +
      "}"
    );
  return JSON.stringify(value);
}
export const oracleFingerprint = Effect.gen(function* () {
  const fs = yield* FileSystem.FileSystem;
  return {
    dockerfile: hash(yield* fs.readFile(dockerfile)),
    profile: hash(
      profileJson({
        env: golemEnv,
        oracle_env: oracleEnv,
        user: oracleUser,
        setup: "writable-root-v2",
      }),
    ),
  };
});
const readGolden = (corpus: string) =>
  Effect.gen(function* () {
    const fs = yield* FileSystem.FileSystem,
      path = join(here, "goldens", `${corpus}.json`);
    return (yield* fs.exists(path))
      ? yield* readJson(path, Golden)
      : ({ oracle: null, cases: new Map() } satisfies Golden);
  });
const goldensFor = (cases: readonly Case[]) =>
  Effect.forEach([...new Set(cases.map((c) => c.corpus))], (corpus) =>
    readGolden(corpus).pipe(Effect.map((data) => [corpus, data] as const)),
  ).pipe(Effect.map((entries) => new Map(entries)));
const compile = (args: Args, base: string) =>
  run(
    [
      args.wasmtime,
      "compile",
      ...wasmtimeOptions,
      "-o",
      join(base, "shell.cwasm"),
      absolute(args.wasm!),
    ],
    120,
    true,
  ).pipe(Effect.as(join(base, "shell.cwasm")));
export const runOurs = (c: Case, compiled: string, wasmtime: string) =>
  Effect.scoped(
    Effect.gen(function* () {
      const fs = yield* FileSystem.FileSystem,
        root = yield* temporary(),
        report = yield* temporary();
      if (!c.options.no_tmp) yield* fs.makeDirectory(join(root, "tmp"));
      const command = [
        wasmtime,
        "run",
        ...wasmtimeOptions,
        "--allow-precompiled",
        "--dir",
        `${root}::/`,
        "--dir",
        `${report}::/.conformance`,
      ];
      for (const [key, value] of Object.entries(golemEnv))
        command.push("--env", `${key}=${value}`);
      command.push(compiled, "--status-file", "/.conformance/status");
      const calls = splitCalls(c.script);
      command.push(
        ...(calls.length > 1
          ? ["--calls", ...calls.map((call) => call.script)]
          : [c.script]),
      );
      const result = yield* run(command, c.options.timeout ?? 15);
      const status = yield* fs
        .readFileString(join(report, "status"))
        .pipe(
          Effect.mapError(
            (error) =>
              new Error(
                `missing shell status report: ${error}; stderr=${JSON.stringify(result.stderr)}`,
              ),
          ),
        );
      return {
        ...result,
        status: yield* attempt(() => wasmStatus(result.status, status)),
      };
    }),
  );

const buildOracle = (image: string) =>
  run(
    ["docker", "build", "-q", "-f", dockerfile, "-t", image, here],
    600,
    true,
  );
const oracle = (image: string) =>
  Effect.gen(function* () {
    const name = `golem-conformance-${randomUUID().slice(0, 12)}`;
    yield* Effect.acquireRelease(
      run(
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
          "infinity",
        ],
        120,
        true,
      ),
      () =>
        run(["docker", "rm", "-f", name], 60, true).pipe(
          Effect.catchAll((error) =>
            Console.error(`oracle cleanup ${name}: ${error}`),
          ),
        ),
    );
    const root = (script: string) =>
      run(["docker", "exec", "-u", "0", name, "sh", "-c", script], 60, true);
    const initial = yield* root("chmod 1777 / && ls -A /");
    const baseline = decodeBytes(initial.stdout)
      .toString()
      .split(/\s+/)
      .filter((n) => n && n !== "tmp")
      .join(" ");
    return (c: Case) =>
      Effect.gen(function* () {
        yield* root(
          `kill -9 -1 2>/dev/null; cd / || exit 1; for entry in $(ls -A /); do case " ${baseline} " in *" $entry "*) ;; *) rm -rf "/$entry" ;; esac; done; ` +
            (c.options.no_tmp ? "true" : "mkdir -m 1777 /tmp"),
        );
        let status = 0;
        const stdout: Buffer[] = [],
          stderr: Buffer[] = [];
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
                ([k, v]) => `${k}=${v}`,
              ),
              "bash",
              "-c",
              call.script,
            ],
            c.options.timeout ?? 15,
          );
          status = result.status;
          stdout.push(decodeBytes(result.stdout));
          stderr.push(decodeBytes(result.stderr));
        }
        return {
          status,
          stdout: encodeBytes(Buffer.concat(stdout)),
          stderr: encodeBytes(Buffer.concat(stderr)),
        };
      });
  });
type OracleResult = { outcome: Outcome } | { error: string };
const runOracle = (cases: readonly Case[], args: Args) =>
  Effect.gen(function* () {
    const results = new Map<string, OracleResult>();
    const workers = Math.min(args.jobs, cases.length);
    // Each worker owns one container. Every record pass creates a new set of containers.
    yield* Effect.forEach(
      Array.from({ length: workers }, (_, i) => i),
      (i) =>
        Effect.scoped(
          Effect.gen(function* () {
            const execute = yield* oracle(args["oracle-image"]);
            for (let n = i; n < cases.length; n += workers) {
              const c = cases[n];
              const result = yield* execute(c).pipe(
                Effect.match({
                  onFailure: (error) => ({ error: String(error) }),
                  onSuccess: (outcome) => ({ outcome }),
                }),
              );
              results.set(c.name, result);
            }
          }),
        ),
      { concurrency: "unbounded" },
    );
    return results;
  });

const checkBaseline = (
  args: Args,
  cases: readonly Case[],
  failures: string[],
) =>
  Effect.gen(function* () {
    const path = args["known-failures"];
    if (!path) return failures.length === 0;
    if (args["update-known-failures"]) {
      yield* writeJson(path, failures.sort());
      return true;
    }
    const fs = yield* FileSystem.FileSystem;
    const known = new Set(
      (yield* fs.exists(path))
        ? yield* readJson(path, Schema.Array(Schema.String))
        : [],
    );
    const changes = baselineChanges(
      known,
      new Set(cases.map((c) => c.name)),
      new Set(failures),
    );
    for (const name of changes.new) yield* Console.log(`NEW FAILURE ${name}`);
    for (const name of changes.fixed)
      yield* Console.log(`NOW PASSES ${name} (remove it from ${path})`);
    yield* Console.log(
      `${failures.length} failing; ${changes.new.length} new, ${changes.fixed.length} fixed`,
    );
    return !changes.new.length && !changes.fixed.length;
  });

const compare = (
  args: Args,
  cases: readonly Case[],
  expectation: (c: Case) => Outcome,
) =>
  Effect.scoped(
    Effect.gen(function* () {
      const base = yield* temporary(),
        compiled = yield* compile(args, base);
      const records = yield* Effect.forEach(
        cases,
        (c) =>
          Effect.gen(function* () {
            const expectedResult = yield* attempt(() => expectation(c)).pipe(
              Effect.either,
            );
            let expected: Outcome | null = null,
              actual: Outcome | null = null,
              error: string | null = null;
            if (expectedResult._tag === "Left")
              error = String(expectedResult.left);
            else {
              expected = expectedResult.right;
              const result = yield* runOurs(c, compiled, args.wasmtime).pipe(
                Effect.either,
              );
              if (result._tag === "Left") error = String(result.left);
              else actual = result.right;
            }
            const failed =
              error || (expected && actual && differingField(expected, actual));
            yield* Console.log(
              `${failed ? "FAIL" : "PASS"} ${c.name}${failed ? `: ${error ?? JSON.stringify({ expected, actual })}` : ""}`,
            );
            return failed
              ? {
                  name: c.name,
                  corpus: c.corpus,
                  tags: c.tags,
                  script: c.script,
                  expected,
                  actual,
                  error,
                }
              : null;
          }),
        { concurrency: args.jobs },
      );
      const failures = records
        .filter((r) => r !== null)
        .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
      yield* Console.log(
        `${failures.length} failures of ${cases.length} cases`,
      );
      if (args.report) yield* writeJson(args.report, failures);
      return yield* checkBaseline(
        args,
        cases,
        failures.map((r) => r.name),
      );
    }),
  );

const coverage = (args: Args, cases: readonly Case[]) =>
  Effect.scoped(
    Effect.gen(function* () {
      const counts = new Map(Object.keys(featureTable).map((tag) => [tag, 0]));
      const problems: string[] = [];
      for (const c of cases) {
        const { found, unknown } = credited(c);
        for (const tag of unknown)
          problems.push(`${c.name}: unknown tag ${tag}`);
        for (const tag of found) counts.set(tag, (counts.get(tag) ?? 0) + 1);
      }
      const base = yield* temporary(),
        compiled = yield* compile(args, base);
      const result = yield* runOurs(
        {
          name: "compgen",
          script: "compgen -b; echo --; compgen -c; echo --; compgen -k",
          corpus: "coverage",
          tier: "pr",
          tags: [],
          options: {},
        },
        compiled,
        args.wasmtime,
      );
      if (result.status)
        return yield* Effect.fail(
          new Error(`compgen failed: ${JSON.stringify(result.stderr)}`),
        );
      const parts = decodeBytes(result.stdout).toString().split("--\n");
      if (parts.length !== 3)
        return yield* Effect.fail(new Error("invalid compgen output"));
      const [builtins, all, keywords] = parts.map(
        (part) => new Set(part.split(/\s+/).filter(Boolean)),
      );
      const commands = new Set(
        [...all].filter((n) => !builtins.has(n) && !keywords.has(n)),
      );
      for (const [registered, listed, kind] of [
        [
          builtins,
          new Set([...features.builtins, ...features.builtinReplacements]),
          "builtin",
        ],
        [
          commands,
          new Set([
            ...features.commands.filter(
              (n) => !features.builtinReplacements.includes(n),
            ),
            ...features.fixtures,
          ]),
          "command",
        ],
      ] as const) {
        for (const n of registered)
          if (!listed.has(n))
            problems.push(
              `${kind} ${n} is registered but not in features.json`,
            );
        for (const n of listed)
          if (!registered.has(n))
            problems.push(
              `${kind} ${n} is in features.json but not registered`,
            );
      }
      const gaps = [...counts].filter(
        ([tag, count]) => !count && !(tag in features.notApplicable),
      );
      for (const [tag, count] of [...counts].sort())
        yield* Console.log(
          `${tag}  ${tag in features.notApplicable ? "n/a" : count}`,
        );
      for (const problem of problems) yield* Console.log(`PROBLEM ${problem}`);
      for (const [tag] of gaps)
        yield* Console.log(`GAP ${tag} - ${featureTable[tag]}`);
      yield* Console.log(`${gaps.length} gaps; ${problems.length} problems`);
      return !gaps.length && !problems.length;
    }),
  );

export const program = (argv: string[]) =>
  Effect.gen(function* () {
    const args = yield* attempt(() => argumentsFor(argv)),
      all = yield* loadCases();
    const cases = all.filter(
      (c) =>
        ((!args.case.length && !args.prefix.length) ||
          args.case.includes(c.name) ||
          args.prefix.some((p) => c.name.startsWith(p))) &&
        (args.command !== "check" || args.tier === "all" || c.tier === "pr"),
    );
    if (!cases.length)
      return yield* Effect.fail(new Error("no cases selected"));
    if (args.command === "coverage") return yield* coverage(args, cases);
    const fingerprint = yield* oracleFingerprint,
      goldens = yield* goldensFor(cases);
    if (args.command === "check")
      return yield* compare(args, cases, (c) => {
        const golden = goldenFor(c, goldens.get(c.corpus)!, fingerprint),
          redundant = redundantFixture(c, golden);
        if (redundant) throw new Error(redundant);
        return expectedFor(c, golden);
      });
    yield* buildOracle(args["oracle-image"]);
    if (args.command === "live") {
      const results = yield* runOracle(
        cases.filter((c) => !c.expected),
        args,
      );
      return yield* compare(args, cases, (c) => {
        if (c.expected) return expectedFor(c, c.expected);
        const result = results.get(c.name);
        if (!result || "error" in result)
          throw new Error(
            result && "error" in result ? result.error : "oracle did not run",
          );
        return expectedFor(c, result.outcome);
      });
    }
    if (args.command === "stale") {
      const needed = cases.filter((c) => !c.expected && !c.oracleRace),
        results = yield* runOracle(needed, args);
      let stale = 0;
      for (const c of needed) {
        const result = results.get(c.name);
        const comparison = yield* attempt(() => {
          const golden = goldenFor(c, goldens.get(c.corpus)!, fingerprint);
          if (!result || "error" in result)
            throw new Error(
              result && "error" in result ? result.error : "oracle did not run",
            );
          const field = differingField(golden, result.outcome);
          if (field)
            throw new Error(
              `oracle now answers ${JSON.stringify(result.outcome)} (${field})`,
            );
        }).pipe(Effect.either);
        if (comparison._tag === "Left") {
          stale++;
          yield* Console.log(`STALE ${c.name}: ${comparison.left}`);
        }
      }
      yield* Console.log(`${stale} stale of ${needed.length} goldens`);
      return stale === 0;
    }
    const runs = yield* Effect.forEach(Array.from({ length: args.runs }), () =>
      runOracle(cases, args),
    );
    let failures = 0,
      recorded = 0;
    for (const corpus of new Set(cases.map((c) => c.corpus))) {
      const old = goldens.get(corpus)!;
      const entries = new Map<string, Outcome & { script: string }>(
        old.oracle?.dockerfile === fingerprint.dockerfile &&
        old.oracle?.profile === fingerprint.profile
          ? old.cases
          : [],
      );
      for (const c of cases.filter((c) => c.corpus === corpus)) {
        const values = runs.map((run) => run.get(c.name));
        const outcomes = values.flatMap((v) =>
          v && "outcome" in v ? [v.outcome] : [],
        );
        if (outcomes.length !== args.runs) {
          failures++;
          yield* Console.log(`ERROR ${c.name}: ${JSON.stringify(values)}`);
          continue;
        }
        const field = outcomes
          .slice(1)
          .map((o) => differingField(outcomes[0], o))
          .find(Boolean);
        if (field) {
          failures++;
          yield* Console.log(
            `NONDETERMINISTIC ${c.name}: ${field} differs between runs`,
          );
          continue;
        }
        entries.set(c.name, { ...outcomes[0], script: hash(c.script) });
        recorded++;
      }
      const names = new Set(all.map((c) => c.name));
      const data = {
        oracle: fingerprint,
        cases: Object.fromEntries(
          [...entries]
            .filter(([n]) => names.has(n))
            .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)),
        ),
      };
      yield* writeJson(join(here, "goldens", `${corpus}.json`), data);
    }
    yield* Console.log(`recorded ${recorded} of ${cases.length} cases`);
    return failures === 0;
  });
if (import.meta.main) main(program(process.argv.slice(2)));
