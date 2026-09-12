import { z } from "zod"
import { defineAgent, method } from "@golemcloud/golem-ts-sdk"
import { EffectFixture } from "effect-fixture-guest-client"

export const TsPeer = defineAgent({
  name: "TsPeer",
  id: { name: z.string() },
  methods: {
    echo: method({
      input: { value: z.string() },
      returns: z.object({ language: z.string(), value: z.string() }),
    }),
    callEffect: method({
      input: { tenant: z.string(), requestId: z.string() },
      returns: z.string(),
    }),
  },
})

TsPeer.implement({
  init: ({ id }) => ({ name: id.name }),
  methods: {
    echo({ value }) {
      return { language: "ts", value: `${this.name}:${value}` }
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
  },
})
