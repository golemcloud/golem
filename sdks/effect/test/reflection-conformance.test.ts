import { readFileSync } from "node:fs"
import { describe, expect, it } from "@effect/vitest"
import { SchemaRef, type JsonValue } from "../src/SchemaRef.js"
import { field, t, type SchemaGraph, type SchemaType } from "../src/internal/schema-model/model.js"
import { schemaGraphToWit } from "../src/internal/schema-model/wit.js"

interface ConformanceCase {
  id: string
  operation: "roundtrip" | "reject" | "json-schema" | "semantic"
  fixture: string
  input?: JsonValue
  inputs?: JsonValue[]
  path?: string
  expected: JsonValue
}

interface ConformanceCorpus {
  version: string
  schemaKinds: string[]
  restrictionKinds: string[]
  caseIds: string[]
  cases: ConformanceCase[]
}

const corpus = JSON.parse(
  readFileSync(
    new URL("../../../test-data/reflection-conformance/v1.json", import.meta.url),
    "utf8",
  ),
) as ConformanceCorpus

const supportedSchemaKinds = [
  "ref",
  "bool",
  "s8",
  "s16",
  "s32",
  "s64",
  "u8",
  "u16",
  "u32",
  "u64",
  "f32",
  "f64",
  "char",
  "string",
  "record",
  "variant",
  "enum",
  "flags",
  "tuple",
  "list",
  "fixed-list",
  "map",
  "option",
  "result",
  "text",
  "binary",
  "path",
  "url",
  "datetime",
  "duration",
  "quantity",
  "union",
  "secret",
  "quota-token",
  "permission-card",
  "future",
  "stream",
] as const

const supportedRestrictionKinds = [
  "numeric-minimum",
  "numeric-maximum",
  "numeric-unit",
  "text-languages",
  "text-min-length",
  "text-max-length",
  "text-regex",
  "binary-mime-types",
  "binary-min-bytes",
  "binary-max-bytes",
  "path-direction",
  "path-kind",
  "path-mime-types",
  "path-extensions",
  "url-schemes",
  "url-hosts",
  "quantity-base-unit",
  "quantity-suffixes",
  "quantity-minimum",
  "quantity-maximum",
  "union-prefix",
  "union-suffix",
  "union-regex",
  "union-field",
] as const

const ref = (root: SchemaType) => new SchemaRef(schemaGraphToWit({ defs: new Map(), root }))

function fixture(name: string): SchemaRef {
  switch (name) {
    case "s64":
      return ref(t.s64())
    case "constrained-s64":
      return ref(
        t.s64({
          min: { tag: "signed", val: -9_007_199_254_740_993n },
          max: { tag: "signed", val: 9_007_199_254_740_993n },
        }),
      )
    case "u64":
      return ref(t.u64())
    case "binary":
      return ref(t.binary())
    case "duration":
      return ref(t.duration())
    case "quantity":
      return ref(t.quantity({ baseUnit: "m", allowedSuffixes: [] }))
    case "optional-record": {
      const graph: SchemaGraph = {
        defs: new Map([["conformance.optional", { body: t.option(t.string()) }]]),
        root: t.record([
          field("direct", t.option(t.string())),
          field("referenced", t.ref("conformance.optional")),
        ]),
      }
      return new SchemaRef(schemaGraphToWit(graph))
    }
    case "tool-input":
      return ref(
        t.record([
          field("pattern", t.string()),
          field("paths", t.list(t.string())),
          field("ignoreCase", t.option(t.bool())),
        ]),
      )
    case "config-entry":
      return ref(t.record([field("path", t.list(t.string())), field("value", t.s64())]))
    case "constrained-u32":
      return ref(
        t.u32({
          min: { tag: "unsigned", val: 2n },
          max: { tag: "unsigned", val: 10n },
        }),
      )
    case "constrained-f64":
      return ref(
        t.f64({
          min: { tag: "float-bits", val: 0xbff8000000000000n },
          max: { tag: "float-bits", val: 0x4004000000000000n },
        }),
      )
    case "constrained-text":
      return ref(
        t.text({
          languages: ["en", "de"],
          minLength: 2,
          maxLength: 8,
          regex: "^[a-z]+$",
        }),
      )
    case "constrained-binary":
      return ref(
        t.binary({
          mimeTypes: ["image/png", "application/octet-stream"],
          minBytes: 2,
          maxBytes: 4,
        }),
      )
    case "result":
      return ref(t.result(t.string(), t.u32()))
    case "custom-error":
      return ref(
        t.result(t.string(), t.record([field("code", t.string()), field("retryable", t.bool())])),
      )
    default:
      throw new Error(`unknown conformance fixture ${name}`)
  }
}

function atPointer(value: JsonValue, pointer: string): JsonValue {
  if (pointer === "") return value
  return pointer
    .slice(1)
    .split("/")
    .map((part) => part.replaceAll("~1", "/").replaceAll("~0", "~"))
    .reduce<JsonValue>((current, part) => {
      if (current === null || typeof current !== "object" || Array.isArray(current)) {
        throw new Error(`cannot resolve ${pointer}`)
      }
      const next = current[part]
      if (next === undefined) throw new Error(`missing ${pointer}`)
      return next
    }, value)
}

function expectSubset(actual: JsonValue, expected: JsonValue): void {
  if (expected !== null && typeof expected === "object" && !Array.isArray(expected)) {
    expect(actual).not.toBeNull()
    expect(typeof actual).toBe("object")
    expect(Array.isArray(actual)).toBe(false)
    for (const [key, value] of Object.entries(expected)) {
      expect(Object.prototype.hasOwnProperty.call(actual, key)).toBe(true)
      expectSubset((actual as Record<string, JsonValue>)[key], value)
    }
  } else {
    expect(actual).toEqual(expected)
  }
}

function assertSemantic(testCase: ConformanceCase): void {
  const expected = testCase.expected as Record<string, JsonValue>
  switch (testCase.fixture) {
    case "unsupported-leaves": {
      const unsupported = [
        t.secret(t.string()),
        t.quotaToken({}),
        t.permissionCard({ polymorphic: false }),
        t.future(t.string()),
        t.stream(t.string()),
      ]
      expect(unsupported).toHaveLength(expected.count as number)
      for (const type of unsupported) expectSubset(ref(type).toJsonSchema(), expected.schema)
      break
    }
    case "all-kinds":
      expect(supportedSchemaKinds).toEqual(expected.names)
      expect(corpus.schemaKinds).toEqual(expected.names)
      break
    case "all-restrictions":
      expect(supportedRestrictionKinds).toEqual(expected.names)
      expect(corpus.restrictionKinds).toEqual(expected.names)
      break
    case "graph": {
      const referenced = fixture("optional-record")
      const inline = ref(
        t.record([
          field("direct", t.option(t.string())),
          field("referenced", t.option(t.string())),
        ]),
      )
      expect(referenced.packJson({})).toEqual(inline.packJson({}))
      expect(referenced.validateJson({}).success).toBe(true)
      break
    }
    default:
      throw new Error(`unknown semantic conformance fixture ${testCase.fixture}`)
  }
}

function executeCase(testCase: ConformanceCase): void {
  switch (testCase.operation) {
    case "roundtrip": {
      const schema = fixture(testCase.fixture)
      expect(schema.unpackJson(schema.packJson(testCase.input!))).toEqual(testCase.expected)
      break
    }
    case "reject": {
      for (const input of testCase.inputs ?? [testCase.input!]) {
        const schema = fixture(testCase.fixture)
        let packed: ReturnType<typeof schema.packJson>
        try {
          packed = schema.packJson(input)
        } catch {
          expect("invalid-json").toBe((testCase.expected as { readonly kind: string }).kind)
          continue
        }
        if (schema.validateValue(packed).success)
          throw new Error(`accepted ${JSON.stringify(input)}`)
        expect("constraint-violation").toBe((testCase.expected as { readonly kind: string }).kind)
      }
      break
    }
    case "json-schema":
      expectSubset(
        atPointer(fixture(testCase.fixture).toJsonSchema(), testCase.path ?? ""),
        testCase.expected,
      )
      break
    case "semantic":
      assertSemantic(testCase)
      break
    default:
      throw new Error(`unknown conformance operation ${String(testCase.operation)}`)
  }
}

describe("reflection conformance corpus", () => {
  it("has a valid version, unique declared case IDs, and recognized operations", () => {
    expect(corpus.version).toBe("1.0.0")
    const ids = corpus.cases.map((testCase) => testCase.id)
    expect(new Set(ids).size, "duplicate corpus case ID").toBe(ids.length)
    expect(new Set(corpus.caseIds).size, "duplicate declared case ID").toBe(corpus.caseIds.length)
    expect([...ids].sort()).toEqual([...corpus.caseIds].sort())
    const operations = new Set(["roundtrip", "reject", "json-schema", "semantic"])
    for (const testCase of corpus.cases) {
      expect(operations.has(testCase.operation), testCase.id).toBe(true)
    }
  })

  for (const testCase of corpus.cases) it(testCase.id, () => executeCase(testCase))
})
