import { z } from "zod"
import {
  acquireQuotaToken,
  AgentStream,
  defineAgent,
  method,
  ok,
  reflection,
  s,
  toolDefinition,
} from "@golemcloud/golem-ts-sdk"
import { EffectFixture } from "effect-fixture-guest-client"
import { EffectRichFixture } from "effect-rich-fixture-guest-client"
import { EffectSnapshotFixture } from "effect-snapshot-fixture-guest-client"
import { EffectCrossStreamingClient } from "effect-cross-streaming-tool-guest-client"

toolDefinition("ts-cross-streaming")
  .body((body) =>
    body
      .positional("label", z.string())
      .stdin({ required: true })
      .stdout({ required: true })
      .returns(z.string()),
  )
  .implement({
    "ts-cross-streaming": async ({ label }, context) => {
      const reader = context.stdin.getReader()
      const writer = context.stdout.getWriter()
      await writer.write(new TextEncoder().encode(`ts:${label}:`))
      while (true) {
        const item = await reader.read()
        if (item.done) break
        await writer.write(
          new TextEncoder().encode(new TextDecoder().decode(item.value).toUpperCase()),
        )
      }
      await writer.close()
      return ok(`ts-ok:${label}`)
    },
  })

export const TsEphemeralPeer = defineAgent({
  name: "TsEphemeralPeer",
  mode: "ephemeral",
  id: { request: z.string() },
  methods: { echo: method({ input: { value: z.string() }, returns: z.string() }) },
})

TsEphemeralPeer.implement({
  init: ({ id }) => ({ request: id.request }),
  methods: {
    echo({ value }) {
      return `ephemeral:${this.request}:${value}`
    },
  },
})

export const TsPeer = defineAgent({
  name: "TsPeer",
  id: { name: z.string() },
  methods: {
    echo: method({
      input: { value: z.string() },
      returns: z.object({ language: z.string(), value: z.string() }),
    }),
    nonfinite: method({ input: { kind: z.string() }, returns: z.number() }),
    nestedStream: method({
      input: {
        prefix: z.string(),
        items: s.stream(z.object({ id: z.number(), values: z.array(z.number()) })),
      },
      returns: z.object({
        items: s.stream(z.object({ id: z.number(), values: z.array(z.number()) })),
      }),
    }),
    markScheduled: method({ input: {}, returns: z.void() }),
    scheduledCount: method({ input: {}, returns: z.number() }),
    echoQuota: method({ input: { token: s.quotaToken() }, returns: s.quotaToken() }),
    callEffect: method({
      input: { tenant: z.string(), requestId: z.string() },
      returns: z.string(),
    }),
    callEffectFailure: method({
      input: { tenant: z.string(), requestId: z.string() },
      returns: z.string(),
    }),
    callEffectStream: method({ input: { tenant: z.string() }, returns: z.string() }),
    callEffectTool: method({ input: { payload: z.string() }, returns: z.string() }),
    quotaThroughEffect: method({ input: { tenant: z.string() }, returns: z.string() }),
    richCorpusThroughEffect: method({ input: { tenant: z.string() }, returns: z.string() }),
    snapshotAdd: method({
      input: { tenant: z.string(), by: z.number() },
      returns: z.number(),
    }),
    snapshotValue: method({ input: { tenant: z.string() }, returns: z.number() }),
    schemaNodesThroughEffect: method({ input: { tenant: z.string() }, returns: z.string() }),
  },
})

TsPeer.implement({
  init: ({ id }) => ({ name: id.name, scheduled: 0 }),
  methods: {
    echo({ value }) {
      return { language: "ts", value: `${this.name}:${value}` }
    },
    nonfinite({ kind }) {
      return kind === "nan"
        ? Number.NaN
        : kind === "positive"
          ? Number.POSITIVE_INFINITY
          : Number.NEGATIVE_INFINITY
    },
    nestedStream({ prefix, items }) {
      return {
        items: AgentStream.from(
          (async function* () {
            for await (const item of items) {
              yield { id: item.id * 100, values: item.values.map((value) => value - prefix.length) }
            }
          })(),
        ),
      }
    },
    markScheduled() {
      this.scheduled += 1
    },
    scheduledCount() {
      return this.scheduled
    },
    echoQuota({ token }) {
      return token
    },
    async callEffect({ tenant, requestId }) {
      const result = await EffectFixture.getWithConfig(tenant, "ts-override").transform({
        header: { requestId, flags: [true, false] },
        items: [{ sku: "TS", quantities: [2, 3] }],
      })
      return "ok" in result
        ? `${result.ok.summary}:${result.ok.accepted[0]?.total}`
        : `error:${result.err.code}`
    },
    async callEffectFailure({ tenant, requestId }) {
      const result = await EffectFixture.getWithConfig(tenant, "ts-failure").transform({
        header: { requestId, flags: [false] },
        items: [],
      })
      return "err" in result
        ? `${result.err.code}:${result.err.requestId}`
        : `unexpected:${result.ok.summary}`
    },
    async callEffectStream({ tenant }) {
      let pulled = 0
      let closed = false
      const source = AgentStream.from(
        (async function* () {
          try {
            for (let id = 1; id <= 2050; id++) {
              pulled++
              yield { id, values: id === 1 ? [1, 4] : id === 2 ? [8] : [99] }
            }
          } finally {
            closed = true
          }
        })(),
      )
      const output = await EffectFixture.get(tenant).transformStream({
        prefix: "typescript",
        items: source,
      })
      const values = []
      const reader = output.items[Symbol.asyncIterator]()
      for (let index = 0; index < 2; index++) {
        const item = await reader.next()
        if (!item.done) values.push(item.value)
      }
      const readerClosed =
        reader.return === undefined ? false : (await reader.return()).done === true
      const stoppedEarly = pulled < 2050
      for (let attempt = 0; attempt < 100 && !closed; attempt++) {
        await new Promise((resolve) => setTimeout(resolve, 10))
      }
      return `${JSON.stringify(values)}:${readerClosed}:${stoppedEarly}:${closed}`
    },
    async callEffectTool({ payload }) {
      const stdin = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(payload))
          controller.close()
        },
      })
      const { result, stdout } = await EffectCrossStreamingClient.newClient()
        .effect_cross_streaming(this.name, stdin)
        .collect()
      return `${result}|${new TextDecoder().decode(stdout)}`
    },
    async quotaThroughEffect({ tenant }) {
      const token = acquireQuotaToken("cross-sdk-quota", 2n)
      const returned = await EffectFixture.get(tenant).reserveForwardedQuota(token)
      returned.reserve(1n).unwrap().commit(1n)
      let oldRejected = false
      try {
        token.reserve(0n)
      } catch {
        oldRejected = true
      }
      return `reserved-after-effect:${oldRejected}`
    },
    async richCorpusThroughEffect({ tenant }) {
      const result = await EffectRichFixture.get(tenant).transformRichCorpus({
        u8: 254,
        u16: 65530,
        u32: 2_147_483_647,
        u64: 18_446_744_073_709_551_615n,
        s8: -120,
        s16: -32_000,
        s32: -2_000_000_000,
        s64: -9_223_372_036_854_775_808n,
        f32: 1.5,
        f64: -2.25,
        char: "ß",
        enumValue: "violet",
        flags: { read: true, write: false, admin: false },
        variant: { tag: "case0", val: "variant-input" },
        tuple: ["tuple", 44, false],
        list: [1, 65_000],
        fixed: ["left", "right"],
        map: new Map([["answer", 42]]),
        option: undefined,
        result: { ok: 31 },
        path: "notes.txt",
        url: "https://golem.cloud/corpus",
        datetime: "2024-02-29T12:34:56.123456789Z",
        duration: 9_223_372_036_854_775_000n,
        choice: { tag: "name", val: "name:alice" },
      })
      const expected = {
        u8: 255,
        u16: 65_530,
        u32: 2_147_483_647,
        u64: 18_446_744_073_709_551_613n,
        s8: -120,
        s16: -32_000,
        s32: -2_000_000_000,
        s64: -9_223_372_036_854_775_805n,
        f32: 1.5,
        f64: -2.25,
        char: "λ",
        enumValue: "cyan",
        flags: { read: true, write: true, admin: true },
        variant: { tag: "case0", val: "variant-input" },
        tuple: ["tuple!", 40, true],
        list: [11, 65_010],
        fixed: ["right", "left"],
        map: [["answer!", -42]],
        option: "effect-option",
        result: { err: "effect:31" },
        path: "effect-notes.txt",
        url: "https://golem.cloud/corpus",
        datetime: "2024-02-29T12:34:56.123456789Z",
        duration: 9_223_372_036_854_775_009n,
        choice: { tag: "point", val: { kind: "point", x: 17 } },
      }
      const actual = {
        ...result,
        map: [...result.map],
      }
      for (const [key, value] of Object.entries(expected)) {
        const actualValue = actual[key as keyof typeof actual]
        if (
          JSON.stringify(actualValue, (_, item) =>
            typeof item === "bigint" ? `${item}n` : item,
          ) !== JSON.stringify(value, (_, item) => (typeof item === "bigint" ? `${item}n` : item))
        ) {
          return `mismatch:${key}`
        }
      }
      return "rich-corpus-ok"
    },
    async snapshotAdd({ tenant, by }) {
      return EffectSnapshotFixture.get(tenant).add(by)
    },
    async snapshotValue({ tenant }) {
      return EffectSnapshotFixture.get(tenant).value()
    },
    async schemaNodesThroughEffect({ tenant }) {
      const type = reflection.getAgentType("EffectSchemaFixture")
      if (!type || type.mode !== "durable") return "missing-schema-fixture"
      const client = type.client.get({ tenant })
      const invoke = (name: string, value: unknown) =>
        client.method(name).invokeValue({
          tag: "record",
          fields: [value],
        } as Parameters<ReturnType<typeof client.method>["invokeValue"]>[0])
      const text = await invoke("echoText", {
        tag: "variant",
        caseIndex: 0,
        payload: { tag: "text", text: "hello", language: "en" },
      })
      const binary = await invoke("echoBinary", {
        tag: "variant",
        caseIndex: 0,
        payload: {
          tag: "binary",
          bytes: new Uint8Array([0, 127, 255]),
          mimeType: "application/octet-stream",
        },
      })
      const quantity = await invoke("echoQuantity", {
        tag: "quantity",
        value: { mantissa: 12345n, scale: -3, unit: "kg" },
      })
      const recursive = await invoke("echoRecursive", {
        tag: "record",
        fields: [
          { tag: "string", value: "root" },
          {
            tag: "list",
            elements: [
              {
                tag: "record",
                fields: [
                  { tag: "string", value: "leaf" },
                  { tag: "list", elements: [] },
                ],
              },
            ],
          },
        ],
      })
      const field = (result: Awaited<ReturnType<typeof invoke>>) => result.value
      const textValue = field(text)
      const textOk =
        textValue?.tag === "variant" &&
        textValue.caseIndex === 0 &&
        textValue.payload?.tag === "text" &&
        textValue.payload.text === "hello"
      const binaryValue = field(binary)
      const binaryOk =
        binaryValue?.tag === "variant" &&
        binaryValue.caseIndex === 0 &&
        binaryValue.payload?.tag === "binary" &&
        binaryValue.payload.mimeType === "application/octet-stream" &&
        [...binaryValue.payload.bytes].join(",") === "0,127,255"
      const quantityValue = field(quantity)
      const quantityOk =
        quantityValue?.tag === "quantity" &&
        quantityValue.value.mantissa === 12345n &&
        quantityValue.value.scale === -3 &&
        quantityValue.value.unit === "kg"
      const recursiveValue = field(recursive)
      const recursiveOk =
        recursiveValue?.tag === "record" &&
        recursiveValue.fields[0]?.tag === "string" &&
        recursiveValue.fields[0].value === "root" &&
        recursiveValue.fields[1]?.tag === "list" &&
        recursiveValue.fields[1].elements.length === 1
      return `text:${textOk}|binary:${binaryOk}|quantity:${quantityOk}|recursive:${recursiveOk}`
    },
  },
})
