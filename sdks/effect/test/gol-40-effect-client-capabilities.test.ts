import { readFile } from "node:fs/promises"
import { describe, expect, it } from "vitest"
import { Effect, Layer, Schema } from "effect"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { defineConfig } from "../src/Config.js"
import { ConfigClient } from "../src/host/ConfigClient.js"
import { SecretsClient } from "../src/host/SecretsClient.js"
import { compile } from "../src/WitCodec.js"
import * as Quota from "../src/Quota.js"
import * as WitTypes from "../src/WitTypes.js"
import { resetTools } from "../src/internal/tool/model.js"
import {
  exchangeQuota,
  installQuotaExchange,
  quotaEvents,
  quotaLayer,
} from "./fixtures/gol-40-effect-client-capabilities.js"

describe("GOL-40 Effect client capability acceptance", () => {
  it("CLIENT-SECRET transfers a config-borrowed secret through the public secret schema", async () => {
    const SecretConfig = defineConfig("Gol40SecretConfig", {
      secret: Schema.Redacted(Schema.String),
    })
    const compiled = await Effect.runPromise(SecretConfig.__compile())
    const raw = {} as CoreTypes.Secret
    const shape = await Effect.runPromise(
      compiled.buildShape().pipe(
        Effect.provide(
          Layer.mergeAll(
            Layer.succeed(
              ConfigClient,
              ConfigClient.of({
                getConfigValue: () => ({
                  root: 0,
                  valueNodes: [{ tag: "secret-value", val: raw }],
                }),
              }),
            ),
            Layer.succeed(SecretsClient, {} as never),
          ),
        ),
      ),
    )
    const borrowed = await Effect.runPromise(
      Effect.gen(function* () {
        const config = yield* SecretConfig
        return yield* config.secret.borrow
      }).pipe(Effect.provideService(SecretConfig, shape as never)),
    )
    const codec = await Effect.runPromise(compile(WitTypes.Secret(Schema.String)))

    await expect(Effect.runPromise(codec.encode(borrowed as never))).resolves.toMatchObject({
      valueNodes: [{ tag: "secret-value" }],
    })
  })

  it("CLIENT-QUOTA acquires, exchanges twice, reserves, and preserves affine ownership", async () => {
    resetTools()
    quotaEvents.length = 0
    installQuotaExchange()
    const observation = await Effect.runPromise(
      Effect.gen(function* () {
        const original = yield* Quota.acquireQuotaToken("matrix-capacity", 2n)
        const first = yield* exchangeQuota(original)
        const second = yield* exchangeQuota(first.token)
        const returnedUsable = yield* Quota.withReservation(second.token, 1n, () =>
          Effect.succeed({ used: 1n, value: true }),
        )
        const originalConsumed = yield* Quota.withReservation(original, 0n, () =>
          Effect.succeed({ used: 0n, value: false }),
        ).pipe(Effect.catch(() => Effect.succeed(true)))
        return { first, second, returnedUsable, originalConsumed }
      }).pipe(Effect.provide(quotaLayer)),
    )

    expect(observation.first.reserved).toBe(true)
    expect(observation.second.reserved).toBe(true)
    expect(observation.returnedUsable).toBe(true)
    expect(observation.originalConsumed).toBe(true)
    expect(quotaEvents).toEqual([
      "acquire:matrix-capacity",
      "reserve:matrix-capacity:1",
      "commit:1",
      "reserve:matrix-capacity:1",
      "commit:1",
      "reserve:matrix-capacity:1",
      "commit:1",
    ])
  })

  it("CLIENT-PERMISSION does not add wallet or derive authority imports", async () => {
    const world = await readFile(new URL("../wit/main.wit", import.meta.url), "utf8")
    expect(world).not.toContain("import golem:permissions/wallet@0.1.0;")
    expect(world).not.toContain("import golem:permissions/derive@0.1.0;")
  })
})
