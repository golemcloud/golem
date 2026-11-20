import { createHash } from "node:crypto";
import { Schema } from "effect";
import features from "./features.json";

export const Bytes = Schema.Union(
  Schema.Struct({ text: Schema.String }),
  Schema.Struct({ base64: Schema.String }),
);
export type Bytes = typeof Bytes.Type;
export const Outcome = Schema.Struct({
  status: Schema.Int.pipe(Schema.between(0, 255)),
  stdout: Bytes,
  stderr: Bytes,
});
export type Outcome = typeof Outcome.Type;
const Reason = Schema.String.pipe(Schema.minLength(1));
export const CaseData = Schema.Struct({
  name: Reason,
  script: Schema.String,
  tags: Schema.Array(Schema.String),
  options: Schema.Struct({
    no_tmp: Schema.optional(Schema.Boolean),
    timeout: Schema.optional(Schema.Number.pipe(Schema.positive())),
  }),
  expected: Schema.optional(
    Schema.Struct({ ...Outcome.fields, reason: Reason }),
  ),
  expectedStderr: Schema.optional(
    Schema.Struct({ value: Bytes, reason: Reason }),
  ),
  oracleRace: Schema.optional(Reason),
});
export const Corpus = Schema.Struct({
  tier: Schema.Literal("pr", "sweep"),
  description: Schema.String,
  cases: Schema.Array(CaseData),
});
export type Case = typeof CaseData.Type & {
  corpus: string;
  tier: "pr" | "sweep";
};
export const Fingerprint = Schema.Struct({
  dockerfile: Schema.String,
  profile: Schema.String,
});
export type Fingerprint = typeof Fingerprint.Type;
export const Golden = Schema.Struct({
  oracle: Schema.NullOr(Fingerprint),
  cases: Schema.transform(
    Schema.Object,
    Schema.Map({
      key: Schema.String,
      value: Schema.Struct({ ...Outcome.fields, script: Schema.String }),
    }),
    { strict: false, decode: Object.entries, encode: Object.fromEntries },
  ),
});
export type Golden = typeof Golden.Type;

export const hash = (data: string | Uint8Array) =>
  createHash("sha256").update(data).digest("hex").slice(0, 16);
export const decodeBytes = (value: Bytes): Buffer =>
  "text" in value
    ? Buffer.from(value.text)
    : Buffer.from(value.base64, "base64");
export function encodeBytes(data: Uint8Array): Bytes {
  try {
    return {
      text: new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
        data,
      ),
    };
  } catch {
    return { base64: Buffer.from(data).toString("base64") };
  }
}
export function differingField(a: Outcome, b: Outcome) {
  if (a.status !== b.status) return "status";
  if (!decodeBytes(a.stdout).equals(decodeBytes(b.stdout))) return "stdout";
  if (!decodeBytes(a.stderr).equals(decodeBytes(b.stderr))) return "stderr";
  return undefined;
}
export function wasmStatus(processStatus: number, report: string): number {
  if (!/^\s*[+-]?\d+\s*$/.test(report))
    throw new Error("missing or invalid shell status report");
  const status = Number(report);
  if (!Number.isInteger(status) || status < 0 || status > 255)
    throw new Error(`shell status is outside the u8 range: ${report}`);
  if (processStatus !== status)
    throw new Error(
      `WASI process status ${processStatus} disagrees with reported shell status ${status}`,
    );
  return status;
}
export function splitCalls(script: string) {
  const calls: { cwd: string; script: string }[] = [];
  let cwd = "/",
    current: string[] = [];
  for (const line of script.split("\n")) {
    if (line === "#--call--" || line.startsWith("#--call-- ")) {
      calls.push({ cwd, script: current.join("\n") });
      cwd = line.slice(10).trim() || "/";
      current = [];
    } else current.push(line);
  }
  calls.push({ cwd, script: current.join("\n") });
  return calls;
}
export function expectedFor(c: Case, oracle: Outcome): Outcome {
  const result = c.expected ?? oracle;
  return {
    status: result.status,
    stdout: result.stdout,
    stderr: c.expected
      ? result.stderr
      : (c.expectedStderr?.value ?? result.stderr),
  };
}
export function goldenFor(
  c: Case,
  data: Golden,
  fingerprint: Fingerprint,
): Outcome {
  if (
    data.oracle?.dockerfile !== fingerprint.dockerfile ||
    data.oracle?.profile !== fingerprint.profile
  )
    throw new Error(
      `goldens for ${c.corpus} were recorded with another oracle; re-record`,
    );
  const entry = data.cases.get(c.name);
  if (!entry)
    throw new Error("no golden; run conformance.ts record for this case");
  if (entry.script !== hash(c.script))
    throw new Error("golden is for an older script; re-record");
  return { status: entry.status, stdout: entry.stdout, stderr: entry.stderr };
}
export function redundantFixture(c: Case, oracle: Outcome) {
  if (c.expected && !differingField(c.expected, oracle))
    return "its EXPECTED fixture matches the oracle; remove the fixture";
  if (
    c.expectedStderr &&
    decodeBytes(c.expectedStderr.value).equals(decodeBytes(oracle.stderr))
  )
    return "its EXPECTED_STDERR fixture matches the oracle; remove the fixture";
}

export const featureTable: Record<string, string> = { ...features.language };
for (const name of features.builtins)
  featureTable[`builtin.${name}`] = `the ${name} builtin`;
for (const name of features.commands) {
  featureTable[`cmd.${name}`] = `the ${name} command`;
  featureTable[`cmd.${name}.error`] = `an error path of ${name}`;
}
for (const tag of Object.keys(features.notApplicable))
  featureTable[tag] ??= "not applicable";
const detectors = features.detectors.map(
  ([tag, pattern]) => [tag, new RegExp(pattern, "mu")] as const,
);
const commandStart = new RegExp(features.commandStart, "gmu");
export function credited(c: Case) {
  const unknown = c.tags.filter(
    (tag) =>
      !Object.hasOwn(featureTable, tag) && !features.qualifiers.includes(tag),
  );
  const found = new Set(
    c.tags.filter((tag) => Object.hasOwn(featureTable, tag)),
  );
  for (const [tag, pattern] of detectors)
    if (pattern.test(c.script)) found.add(tag);
  for (const match of c.script.matchAll(commandStart)) {
    const word = match[1];
    if (features.builtins.includes(word)) found.add(`builtin.${word}`);
    else if (features.commands.includes(word)) found.add(`cmd.${word}`);
  }
  const colon = c.name.indexOf(":");
  if (colon >= 0) {
    const prefix = c.name.slice(0, colon),
      rest = c.name
        .slice(colon + 1)
        .trim()
        .split(/\s+/)[0];
    const command = features.commands.includes(prefix)
      ? prefix
      : features.commands.includes(rest)
        ? rest
        : undefined;
    if (command) {
      found.add(`cmd.${command}`);
      if (
        c.tags.includes("error") ||
        /error|invalid|missing|refus|unsupported|reject|fail|nonexistent|not found|bad |usage|unknown/.test(
          c.name,
        )
      )
        found.add(`cmd.${command}.error`);
    }
  }
  return { found, unknown };
}
export function baselineChanges(
  known: ReadonlySet<string>,
  selected: ReadonlySet<string>,
  failures: ReadonlySet<string>,
) {
  return {
    new: [...failures].filter((n) => !known.has(n)).sort(),
    fixed: [...known].filter((n) => selected.has(n) && !failures.has(n)).sort(),
  };
}
