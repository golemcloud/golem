import { describe, expect, it } from "@effect/vitest"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { SchemaRef } from "../src/SchemaRef.js"
import { field, t, type SchemaGraph } from "../src/internal/schema-model/model.js"
import { schemaGraphToWit } from "../src/internal/schema-model/wit.js"

const ref = (root: SchemaGraph["root"]) =>
  new SchemaRef(schemaGraphToWit({ defs: new Map(), root }))

describe("SchemaRef", () => {
  it("keeps union branch bodies and regex discriminators as separate JSON Schema conditions", () => {
    const schema = ref(
      t.union([
        {
          tag: "named",
          body: t.record([field("kind", t.string()), field("payload", t.string())]),
          discriminator: { tag: "field-equals", val: { fieldName: "kind", literal: "named" } },
          metadata: { aliases: [], examples: [] },
        },
        {
          tag: "pattern",
          body: t.string(),
          discriminator: { tag: "regex", val: "^[a-z]+$" },
          metadata: { aliases: [], examples: [] },
        },
      ]),
    )
    expect(schema.toJsonSchema()).toMatchObject({
      oneOf: [
        {
          allOf: [{ required: ["kind", "payload"] }, { properties: { kind: { const: "named" } } }],
        },
        { allOf: [{ type: "string" }, { pattern: "^[a-z]+$" }] },
      ],
    })
  })

  it("renders numeric restrictions and optional nullable fields", () => {
    const schema = ref(
      t.record([
        field(
          "bounded",
          t.u8({ min: { tag: "unsigned", val: 10n }, max: { tag: "unsigned", val: 20n } }),
        ),
        field("nullable", t.option(t.string())),
      ]),
    )
    expect(schema.toJsonSchema()).toMatchObject({
      required: ["bounded"],
      properties: { bounded: { type: "integer", minimum: 10, maximum: 20 } },
    })
    expect(schema.validateJson({ bounded: 15 }).success).toBe(true)
    expect(schema.validateJson({ bounded: 15, nullable: null }).success).toBe(true)
    expect(
      ref(t.record([field("constructor", t.option(t.string()))])).validateJson({}).success,
    ).toBe(true)
    for (const numeric of [t.f32, t.f64]) {
      expect(
        ref(
          numeric({
            min: { tag: "float-bits", val: 0xbff0000000000000n },
            max: { tag: "float-bits", val: 0x3fe0000000000000n },
          }),
        ).toJsonSchema(),
      ).toMatchObject({ type: "number", minimum: -1, maximum: 0.5 })
    }
  })

  it("rejects native values outside declared restrictions before unpacking", () => {
    const schema = ref(t.u32({ min: { tag: "unsigned", val: 10n } }))
    expect(() =>
      schema.unpackJson({ root: 0, valueNodes: [{ tag: "u32-value", val: 1 }] }),
    ).toThrow(/does not conform/)
  })

  it("renders enforceable rich-value allowlists and canonical base64url bytes", () => {
    const rendered = ref(
      t.record([
        field("text", t.text({ languages: ["en", "de"] })),
        field("binary", t.binary({ mimeTypes: ["image/png"] })),
      ]),
    ).toJsonSchema()
    expect(rendered).toMatchObject({
      properties: {
        text: { properties: { language: { enum: ["en", "de"] } } },
        binary: { properties: { mimeType: { enum: ["image/png"] } } },
      },
    })
    const pattern = (
      rendered as {
        properties: { binary: { properties: { bytes: { pattern: string } } } }
      }
    ).properties.binary.properties.bytes.pattern
    expect(pattern).toBe(
      "^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-][AQgw]|[A-Za-z0-9_-]{2}[AEIMQUYcgkosw048])?$",
    )
    const regex = new RegExp(pattern, "u")
    for (const valid of ["", "AQ", "AQI", "AQID", "-_8"]) expect(regex.test(valid)).toBe(true)
    for (const invalid of ["+/8", "AQ==", "-_9", "A"]) expect(regex.test(invalid)).toBe(false)
  })

  it("matches canonical rich JSON forms, restrictions, and lossless integer ranges", () => {
    const schema = ref(
      t.record([
        field("text", t.text({ minLength: 2, languages: ["en"] })),
        field("binary", t.binary({ minBytes: 2, mimeTypes: ["application/octet-stream"] })),
        field("duration", t.duration()),
        field("quantity", t.quantity({ baseUnit: "m", allowedSuffixes: [] })),
        field("signed", t.s64()),
        field("unsigned", t.u64()),
      ]),
    )
    const valid = {
      text: { text: "ok", language: "en" },
      binary: { bytes: "-_8", mimeType: "application/octet-stream" },
      duration: { nanoseconds: "250000000" },
      quantity: { mantissa: "12", scale: -1, unit: "m" },
      signed: "-9223372036854775808",
      unsigned: "18446744073709551615",
    }

    expect(schema.validateJson(valid).success).toBe(true)
    expect(schema.validateJson({ ...valid, text: { text: "x", language: "en" } }).success).toBe(
      false,
    )
    expect(schema.validateJson({ ...valid, signed: "9223372036854775808" }).success).toBe(false)
    expect(
      schema.validateJson({ ...valid, quantity: { mantissa: 12, scale: -1, unit: "m" } } as never)
        .success,
    ).toBe(false)
    expect(schema.validateJson({ ...valid, duration: { nanoseconds: "-0" } }).success).toBe(false)
    expect(schema.unpackJson(schema.packJson(valid))).toEqual(valid)
    expect(() =>
      ref(t.binary()).unpackJson({
        root: 0,
        valueNodes: [
          {
            tag: "binary-value",
            val: { bytes: new Uint8Array([1]), mimeType: "not a mime" },
          },
        ],
      }),
    ).toThrow(/invalid MIME type/)
  })

  it("validates nested native capabilities without consuming or rewriting them", () => {
    const schema = ref(
      t.record([
        field(
          "items",
          t.list(
            t.tuple([
              t.secret(t.string()),
              t.quotaToken({}),
              t.permissionCard({ polymorphic: false }),
              t.stream(t.string()),
            ]),
          ),
        ),
      ]),
    )
    const secret = {}
    const quota = {}
    const permission = {}
    const stream = {}
    const value: CoreTypes.SchemaValueTree = {
      root: 0,
      valueNodes: [
        { tag: "record-value", val: [1] },
        { tag: "list-value", val: [2] },
        { tag: "tuple-value", val: [3, 4, 5, 6] },
        { tag: "secret-value", val: secret as never },
        { tag: "quota-token-handle", val: quota as never },
        { tag: "permission-card-handle", val: permission as never },
        { tag: "stream-value", val: stream as never },
      ],
    }

    expect(schema.validateValue(value)).toEqual({ success: true, value })
    expect(value.valueNodes.slice(3).map((node) => node.val)).toEqual([
      secret,
      quota,
      permission,
      stream,
    ])
    expect(schema.validateValue({ ...value, root: 1 }).success).toBe(false)
    expect(
      schema.validateValue({
        root: 0,
        valueNodes: value.valueNodes.map((node, index) =>
          index === 2 ? { tag: "tuple-value", val: [3, 4, 5, 5] } : node,
        ),
      }).success,
    ).toBe(false)
  })

  it("rejects schemas without an unambiguous canonical JSON representation", () => {
    for (const unsupported of [
      t.secret(t.string()),
      t.quotaToken({}),
      t.permissionCard({ polymorphic: false }),
      t.future(t.string()),
      t.stream(t.string()),
    ]) {
      const schema = ref(t.record([field("unsupported", unsupported)]))
      const eligibility = schema.jsonEligibility()
      expect(eligibility.success).toBe(false)
      if (!eligibility.success) expect(eligibility.issues[0]?.path).toEqual(["unsupported"])
      expect(schema.toJsonSchema()).toMatchObject({
        properties: { unsupported: { not: {} } },
      })
    }

    const nestedOption = ref(t.option(t.option(t.string())))
    expect(nestedOption.jsonEligibility().success).toBe(false)
    expect(() => nestedOption.toJsonSchema()).toThrow(/None versus Some\(None\)/)

    const indirectlyNullable = ref(
      t.option(
        t.union([
          {
            tag: "nullable",
            body: t.option(t.string()),
            discriminator: { tag: "field-absent", val: "kind" },
            metadata: { aliases: [], examples: [] },
          },
        ]),
      ),
    )
    expect(indirectlyNullable.jsonEligibility().success).toBe(false)
  })

  it("renders only definitions reachable from the selected root", () => {
    const graph: SchemaGraph = {
      defs: new Map([
        ["reachable", { body: t.string() }],
        ["unrelated-capability", { body: t.permissionCard({ polymorphic: false }) }],
      ]),
      root: t.ref("reachable"),
    }
    const schema = new SchemaRef(schemaGraphToWit(graph))

    expect(schema.jsonEligibility().success).toBe(true)
    expect(schema.toJsonSchema()).toMatchObject({
      $ref: "#/$defs/reachable",
      $defs: { reachable: { type: "string" } },
    })
    expect(schema.toJsonSchema()).not.toHaveProperty("$defs.unrelated-capability")
  })

  it("rejects alias-only cycles but accepts productive recursive schemas", () => {
    for (const graph of [
      {
        defs: new Map([["self", { body: t.ref("self") }]]),
        root: t.ref("self"),
      },
      {
        defs: new Map([
          ["left", { body: t.ref("right") }],
          ["right", { body: t.ref("left") }],
        ]),
        root: t.ref("left"),
      },
    ]) {
      const schema = new SchemaRef(schemaGraphToWit(graph))
      expect(schema.jsonEligibility().success).toBe(false)
      expect(() => schema.toJsonSchema()).toThrow(/reference cycle/)
    }

    const recursive = new SchemaRef(
      schemaGraphToWit({
        defs: new Map([
          [
            "node",
            {
              body: t.record([field("value", t.string()), field("next", t.option(t.ref("node")))]),
            },
          ],
        ]),
        root: t.ref("node"),
      }),
    )
    expect(recursive.jsonEligibility().success).toBe(true)
    expect(recursive.validateJson({ value: "last", next: null }).success).toBe(true)
  })

  it("rejects asymmetric nested native trees", () => {
    const schema = ref(t.record([field("pair", t.tuple([t.u8(), t.string()]))]))
    expect(
      schema.validateValue({
        root: 0,
        valueNodes: [
          { tag: "record-value", val: [1] },
          { tag: "tuple-value", val: [2, 3] },
          { tag: "string-value", val: "wrong side" },
          { tag: "u8-value", val: 1 },
        ],
      }).success,
    ).toBe(false)
  })

  it("validates native 64-bit integers outside the lossless JSON range", () => {
    const schema = ref(t.u64())
    const value: CoreTypes.SchemaValueTree = {
      root: 0,
      valueNodes: [{ tag: "u64-value", val: 2n ** 63n }],
    }

    expect(schema.validateValue(value)).toEqual({ success: true, value })
    expect(schema.unpackJson(value)).toBe("9223372036854775808")
  })

  it("renders canonical wide-integer shapes and range metadata", () => {
    expect(ref(t.s64()).toJsonSchema()).toMatchObject({
      type: "string",
      format: "int64",
      pattern: "^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$",
      "x-golem-minimum": "-9223372036854775808",
      "x-golem-maximum": "9223372036854775807",
    })
    expect(ref(t.u64()).toJsonSchema()).toMatchObject({
      type: "string",
      format: "uint64",
      pattern: "^(?:0|[1-9][0-9]*)$",
      "x-golem-minimum": "0",
      "x-golem-maximum": "18446744073709551615",
    })
  })

  it("makes reflection-only unsupported leaves unsatisfiable", () => {
    for (const type of [
      t.secret(t.string()),
      t.quotaToken({}),
      t.permissionCard({ polymorphic: false }),
      t.future(t.string()),
      t.stream(t.string()),
    ]) {
      expect(ref(type).toJsonSchema()).toMatchObject({ not: {} })
    }
  })

  it("renders declared restrictions for canonical wide integers", () => {
    const signed = ref(
      t.s64({ min: { tag: "signed", val: -10n }, max: { tag: "signed", val: 20n } }),
    )
    const unsigned = ref(
      t.u64({ min: { tag: "unsigned", val: 10n }, max: { tag: "unsigned", val: 20n } }),
    )

    expect(signed.validateJson("-11").success).toBe(false)
    expect(unsigned.validateJson("9").success).toBe(false)
    expect(signed.toJsonSchema()).toMatchObject({
      "x-golem-minimum": "-10",
      "x-golem-maximum": "20",
    })
    expect(unsigned.toJsonSchema()).toMatchObject({
      "x-golem-minimum": "10",
      "x-golem-maximum": "20",
    })
  })

  it("distinguishes native float membership from canonical JSON representability", () => {
    for (const [numeric, tag] of [
      [t.f32, "f32-value"],
      [t.f64, "f64-value"],
    ] as const) {
      const schema = ref(numeric())
      const bounded = ref(numeric({ min: { tag: "float-bits", val: 0n } }))
      for (const n of [NaN, Infinity, -Infinity]) {
        const value: CoreTypes.SchemaValueTree = { root: 0, valueNodes: [{ tag, val: n }] }
        expect(schema.validateValue(value).success).toBe(true)
        expect(schema.validateJson(n).success).toBe(false)
        expect(() => schema.unpackJson(value)).toThrow(/finite/)
        expect(bounded.validateValue(value).success).toBe(n === Infinity)
      }
    }
  })

  it("rejects finite JSON numbers that overflow f32", () => {
    const schema = ref(t.f32())

    expect(schema.validateJson(Number.MAX_VALUE).success).toBe(false)
    expect(schema.validateJson(-Number.MAX_VALUE).success).toBe(false)
    expect(schema.validateJson(3.4028234663852886e38).success).toBe(true)
    expect(() =>
      schema.unpackJson({ root: 0, valueNodes: [{ tag: "f32-value", val: Infinity }] }),
    ).toThrow(/finite/)
    expect(() =>
      ref(t.f64()).unpackJson({ root: 0, valueNodes: [{ tag: "f64-value", val: NaN }] }),
    ).toThrow(/finite/)
  })
})
