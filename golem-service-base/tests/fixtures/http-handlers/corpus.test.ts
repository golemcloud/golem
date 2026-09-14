import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import Ajv2020 from "ajv/dist/2020.js";
import { parseTree, type Node } from "jsonc-parser";

// Check fixture integrity, not the not-yet-implemented HTTP runtime.
function loadJson(text: string): unknown {
  const value: unknown = JSON.parse(text);
  // JSON.parse checks syntax but silently overwrites duplicate object members.
  function check(node: Node): void {
    if (node.type === "object") {
      const keys = new Set<string>();
      for (const property of node.children ?? []) {
        const key = property.children![0].value as string;
        if (keys.has(key)) throw new Error(`duplicate JSON key: ${key}`);
        keys.add(key);
      }
    }
    if (node.type === "number" && !Number.isFinite(node.value)) {
      throw new Error("non-JSON number");
    }
    for (const child of node.children ?? []) check(child);
  }
  check(parseTree(text)!);
  return value;
}

function checkBytes(value: unknown, location: string): void {
  if (Array.isArray(value)) {
    value.forEach((item, index) => checkBytes(item, `${location}/${index}`));
  } else if (value !== null && typeof value === "object") {
    for (const [key, item] of Object.entries(value)) {
      if (key.endsWith("_hex")) {
        const items = Array.isArray(item) ? item : [item];
        if (
          !items.every(
            (part) =>
              typeof part === "string" &&
              part.length % 2 === 0 &&
              !/[^0-9a-f]/.test(part),
          )
        ) {
          throw new Error(`${location}/${key}: invalid byte notation`);
        }
      } else {
        checkBytes(item, `${location}/${key}`);
      }
    }
  }
}

interface CorpusCase {
  id: string;
  suite: string;
  rules: string[];
  input: Record<string, unknown>;
  expect: Record<string, unknown>;
}

interface Corpus {
  contract: string;
  cases: CorpusCase[];
}

function readRuleIds(schema: object): Set<string> {
  const ids = (schema as { $defs: { rule: { enum: string[] } } }).$defs.rule
    .enum;
  const rules = new Set(ids);
  if (rules.size !== ids.length) throw new Error("duplicate contract rule ID");
  return rules;
}

const read = (path: string) =>
  readFileSync(new URL(path, import.meta.url), "utf8");
const schema = loadJson(read("./corpus.schema.json")) as object;
// The shared schema uses standard JSON Schema constructs outside Ajv's strict
// authoring subset. Schema validation stays enabled; no coercion or defaults.
const validator = new Ajv2020({ strict: false }).compile<Corpus>(schema);
const corpus = loadJson(read("./corpus.json"));
if (!validator(corpus)) throw new Error(JSON.stringify(validator.errors));
const rules = readRuleIds(schema);
const expectedSuites = new Set(
  (
    schema as { $defs: { case: { properties: { suite: { enum: string[] } } } } }
  ).$defs.case.properties.suite.enum,
);

function validateCorpus(value: unknown): void {
  if (!validator(value))
    throw new Error(`schema: ${JSON.stringify(validator.errors)}`);
  const ids = new Set<string>();
  const coveredRules = new Set<string>();
  const suites = new Set<string>();
  for (const item of value.cases) {
    if (ids.has(item.id)) throw new Error(`duplicate case ID: ${item.id}`);
    ids.add(item.id);
    suites.add(item.suite);
    for (const rule of item.rules) {
      coveredRules.add(rule);
    }
    // OpenAPI documents/examples may legitimately contain keys named body_hex.
    if (item.suite !== "openapi") {
      checkBytes(item.input, `${item.id}/input`);
      checkBytes(item.expect, `${item.id}/expect`);
    }
  }
  const missingSuites = [...expectedSuites].filter(
    (suite) => !suites.has(suite),
  );
  if (missingSuites.length) throw new Error(`missing suites: ${missingSuites}`);
  // C01 describes the checker itself rather than a runtime behavior.
  const uncovered = [...rules].filter(
    (rule) => rule !== "C01" && !coveredRules.has(rule),
  );
  if (uncovered.length)
    throw new Error(`uncovered contract rules: ${uncovered}`);
}

test("corpus integrity", () => {
  validateCorpus(corpus);
  console.log(
    `Validated ${corpus.cases.length} cases in ${expectedSuites.size} suites ` +
      "(corpus integrity only; runtime conformance not executed).",
  );
});

test("duplicate case IDs are rejected", () => {
  const bad = structuredClone(corpus);
  bad.cases.push(structuredClone(bad.cases[0]));
  expect(() => validateCorpus(bad)).toThrow("duplicate case ID");
});

test("missing expectations and unknown suites are rejected", () => {
  const missing = structuredClone(corpus);
  delete (missing.cases[0] as Partial<CorpusCase>).expect;
  expect(() => validateCorpus(missing)).toThrow("schema:");
  const unknown = structuredClone(corpus);
  unknown.cases[0].suite = "routng";
  expect(() => validateCorpus(unknown)).toThrow("schema:");
});

test("unknown rules are rejected", () => {
  const bad = structuredClone(corpus);
  bad.cases[0].rules = ["Z99"];
  expect(() => validateCorpus(bad)).toThrow("schema:");
});

test("missing suites are rejected", () => {
  const bad = structuredClone(corpus);
  bad.cases = bad.cases.filter((item) => item.suite !== "path");
  expect(() => validateCorpus(bad)).toThrow("missing suites");
});

test("uncovered contract rules are rejected", () => {
  const bad = structuredClone(corpus);
  for (const item of bad.cases) {
    item.rules = item.rules.filter((rule) => rule !== "M01");
    if (!item.rules.length) item.rules = ["C01"];
  }
  expect(() => validateCorpus(bad)).toThrow("uncovered contract rules: M01");
});

test("duplicate schema rule IDs are rejected", () => {
  const bad = structuredClone(schema) as {
    $defs: { rule: { enum: string[] } };
  };
  bad.$defs.rule.enum.push("M01");
  expect(() => readRuleIds(bad)).toThrow("duplicate contract rule ID");
});

test("invalid bytes and expected observations are rejected", () => {
  for (const [field, value] of Object.entries({
    body_hex: "0ff",
    status: 65535,
    statuz: 200,
    compiled: [{ Exact: { public_path: [], file_path: 123 } }],
  })) {
    const bad = structuredClone(corpus);
    bad.cases[0].expect[field] = value;
    expect(() => validateCorpus(bad)).toThrow("schema:");
  }
  for (const invalid of ["gg", "61\n", "0ff"]) {
    expect(() =>
      checkBytes({ producer_chunks_hex: ["61", invalid] }, "test"),
    ).toThrow("invalid byte notation");
  }
});

test("non-ASCII header values require hex", () => {
  for (const [headers, valid] of [
    [[["x-test", "\u007f"]], true],
    [[["x-test", "\u0080"]], false],
    [[["x-test", "é"]], false],
    [[{ name: "x-test", value_hex: "80ff" }], true],
  ] as const) {
    const changed = structuredClone(corpus);
    changed.cases.find((item) => item.suite === "envelope")!.input.headers =
      headers;
    if (valid) validateCorpus(changed);
    else expect(() => validateCorpus(changed)).toThrow("schema:");
  }
});

test("OpenAPI examples are opaque", () => {
  const changed = structuredClone(corpus);
  const item = changed.cases.find((item) => item.suite === "openapi")!;
  const providers = item.input.providers as {
    document: Record<string, unknown>;
  }[];
  providers[0].document["x-example"] = {
    body_hex: "ordinary application data, not fixture bytes",
    $ref: "https://never-fetch.invalid/schema",
  };
  validateCorpus(changed);
});

test("strict JSON loading", () => {
  for (const text of [
    '{"x":1,"x":2}',
    '{"nested":{"x":1,"x":2}}',
    '{"x":1,"\\u0078":2}',
    '{"__proto__":1,"__proto__":2}',
    "NaN",
    "Infinity",
    "1e999",
    '{"nested":[1e999]}',
    '{"x":1,}',
    '{/* comment */"x":1}',
    "{} {}",
  ]) {
    expect(() => loadJson(text)).toThrow();
  }
  expect(loadJson('{"a":{"x":1},"b":{"x":2},"numbers":[null,1.5,-2]}')).toEqual(
    { a: { x: 1 }, b: { x: 2 }, numbers: [null, 1.5, -2] },
  );
  expect(loadJson('{"__proto__":{"x":1}}')).toEqual(
    JSON.parse('{"__proto__":{"x":1}}'),
  );
});
