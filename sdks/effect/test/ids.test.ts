import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { defineAgent } from "../src/Agent.js"
import * as Ids from "../src/Ids.js"
import { method } from "../src/Method.js"
import * as Quota from "../src/Quota.js"
import * as Snapshot from "../src/Snapshot.js"
import { toWitCodec } from "../src/WitCodec.js"

const uuid = { highBits: 1n, lowBits: 2n }
const componentId = { uuid }
const agentId = { componentId, agentId: 'Counter("canonical")' }
const accountId = { uuid: { highBits: 3n, lowBits: 4n } }
const environmentId = { uuid: { highBits: 5n, lowBits: 6n } }
const promiseId = { agentId, oplogIdx: 7n }

const roundTripWit = <S extends Schema.Codec<any, any, never, never>>(
  schema: S,
  value: S["Type"],
) =>
  Effect.gen(function* () {
    const wit = yield* toWitCodec(schema)
    const codec = wit.codec as Schema.Codec<S["Type"], any, never, never>
    const encoded = yield* Schema.encodeEffect(codec)(value)
    return yield* Schema.decodeEffect(codec)(encoded)
  })

describe("Ids", () => {
  it.effect("round-trips every canonical identifier through its host WIT representation", () =>
    Effect.gen(function* () {
      expect(yield* roundTripWit(Ids.Uuid, uuid)).toEqual(uuid)
      expect(yield* roundTripWit(Ids.ComponentId, componentId)).toEqual(componentId)
      expect(yield* roundTripWit(Ids.AgentId, agentId)).toEqual(agentId)
      expect(yield* roundTripWit(Ids.AccountId, accountId)).toEqual(accountId)
      expect(yield* roundTripWit(Ids.EnvironmentId, environmentId)).toEqual(environmentId)
      expect(yield* roundTripWit(Ids.PromiseId, promiseId)).toEqual(promiseId)
    }),
  )

  it.effect("supports identifier schemas in method and snapshot definitions", () =>
    Effect.gen(function* () {
      const state = Schema.Struct({
        uuid: Ids.Uuid,
        componentId: Ids.ComponentId,
        agentId: Ids.AgentId,
        accountId: Ids.AccountId,
        environmentId: Ids.EnvironmentId,
        promiseId: Ids.PromiseId,
      })
      const spec = defineAgent({
        name: "CanonicalIdSchemas",
        mode: "durable",
        constructorParams: { componentId: Ids.ComponentId },
        snapshot: Snapshot.define({ schema: state, policy: Snapshot.policy.default }),
        methods: {
          roundTrip: method({
            params: { promiseId: Ids.PromiseId },
            success: Ids.PromiseId,
          }),
        },
      })

      expect(spec.constructorParams.componentId).toBe(Ids.ComponentId)
      expect(spec.methods.roundTrip.params.promiseId).toBe(Ids.PromiseId)
      expect(spec.methods.roundTrip.success).toBe(Ids.PromiseId)

      const snapshotValue = { uuid, componentId, agentId, accountId, environmentId, promiseId }
      const encoded = yield* Schema.encodeEffect(state)(snapshotValue)
      expect(yield* Schema.decodeEffect(state)(encoded)).toEqual(snapshotValue)
    }),
  )

  it("preserves the existing Quota identifier schema aliases", () => {
    expect(Quota.Uuid).toBe(Ids.Uuid)
    expect(Quota.EnvironmentId).toBe(Ids.EnvironmentId)
  })
})
