import { expect, it } from "@effect/vitest"
import { Schema } from "effect"
import { defineAgentClient, method } from "../src/index.js"

const methods = { echo: method({ input: { message: Schema.String }, success: Schema.String }) }

function checkDefinitions() {
  const complete = defineAgentClient({ name: "Echo", id: { name: Schema.String }, methods })
  complete.client.get({ name: "main" })
  complete.agentId({ name: "main" })

  const ephemeral = defineAgentClient({
    name: "Echo",
    id: { name: Schema.String },
    mode: "ephemeral",
    methods,
  })
  ephemeral.client.newPhantom({ name: "main" })
  // @ts-expect-error ephemeral clients cannot perform ordinary durable lookup
  ephemeral.client.get({ name: "main" })
  // @ts-expect-error an ephemeral identity requires a phantom ID
  ephemeral.agentId({ name: "main" })

  const onlyMethods = defineAgentClient({ methods })
  // @ts-expect-error binding-only definitions cannot create identities
  onlyMethods.agentId({ name: "main" })
  // @ts-expect-error binding-only definitions have no lifecycle factories
  onlyMethods.client.get({ name: "main" })

  // @ts-expect-error the name requires an id
  defineAgentClient({ name: "Echo", methods })
  // @ts-expect-error the id requires a name
  defineAgentClient({ id: { name: Schema.String }, methods })
  // @ts-expect-error config is not allowed on a binding-only contract
  defineAgentClient({ methods, config: undefined })
  // @ts-expect-error mode is not allowed on a binding-only contract
  defineAgentClient({ methods, mode: "durable" })
}

it("checks caller-definition shapes at compile time", () => {
  expect(typeof checkDefinitions).toBe("function")
})
