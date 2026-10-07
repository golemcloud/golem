import { describe, expect, it } from "@effect/vitest"
import { Effect, Schema } from "effect"
import { defineAgent } from "../src/Agent.js"
import * as Ids from "../src/Ids.js"
import { method } from "../src/Method.js"
import * as Snapshot from "../src/Snapshot.js"
import { toWitCodec } from "../src/WitCodec.js"
import { schemaGraphToWit, schemaValueToWit } from "../src/internal/schema-model/wit.js"

const uuid = Ids.Uuid.make("00000000-0000-0001-0000-000000000002")
const encodedUuid = { highBits: 1n, lowBits: 2n }
const componentId = { uuid }
const agentId = { componentId, agentId: 'Counter("canonical")' }
const accountId = { uuid: Ids.Uuid.make("00000000-0000-0003-0000-000000000004") }
const environmentId = { uuid: Ids.Uuid.make("00000000-0000-0005-0000-000000000006") }
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
  it.effect("uses the first-class UUID schema type and value nodes", () =>
    Effect.gen(function* () {
      const wit = yield* toWitCodec(Ids.Uuid)
      expect(wit.graph.root.body).toEqual({ tag: "uuid" })
      expect(schemaGraphToWit(wit.graph).typeNodes.at(-1)?.body).toEqual({ tag: "uuid-type" })

      const encoded = yield* Schema.encodeEffect(wit.codec)(uuid)
      expect(encoded).toEqual({ tag: "uuid", value: encodedUuid })
      expect(schemaValueToWit(encoded)).toEqual({
        valueNodes: [{ tag: "uuid-value", val: encodedUuid }],
        root: 0,
      })
      expect(yield* Schema.decodeEffect(wit.codec)(encoded)).toEqual(uuid)
    }),
  )

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
        id: { componentId: Ids.ComponentId },
        snapshotting: Snapshot.define({ schema: state, policy: Snapshot.policy.default }),
        methods: {
          roundTrip: method({
            input: { promiseId: Ids.PromiseId },
            success: Ids.PromiseId,
          }),
        },
      })

      expect(spec.id.componentId).toBe(Ids.ComponentId)
      expect(spec.methods.roundTrip.input.promiseId).toBe(Ids.PromiseId)
      expect(spec.methods.roundTrip.success).toBe(Ids.PromiseId)

      const snapshotValue = { uuid, componentId, agentId, accountId, environmentId, promiseId }
      const encoded = yield* Schema.encodeEffect(state)(snapshotValue)
      expect(yield* Schema.decodeEffect(state)(encoded)).toEqual(snapshotValue)
    }),
  )

  it("exports canonical identifier schemas from the owning module", () => {
    expect(uuid).toBe("00000000-0000-0001-0000-000000000002")
    expect(() => Ids.Uuid.make("00000000-0000-0001-0000-00000000000A")).toThrow()
    expect(Ids.EnvironmentId.fields.uuid).toBe(Ids.Uuid)
  })
})
