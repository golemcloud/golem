import { expect, it } from "@effect/vitest"
import { Schema } from "effect"
import type { AgentId } from "../src/AgentId.js"
import { Client, defineAgentClient, method } from "../src/index.js"

const methods = { echo: method({ input: { message: Schema.String }, success: Schema.String }) }
declare const id: AgentId

function checkDefinitions() {
  const complete = defineAgentClient({ name: "Echo", id: { name: Schema.String }, methods })
  complete.client.upsert({ name: "main" })
  complete.client.create({ name: "main" }, { keyed: true, env: { MODE: "test" } })
  complete.client.getById(id)
  Client.bind(id, complete)
  // @ts-expect-error upsert takes no overrides
  complete.client.upsert({ name: "main" }, { env: { MODE: "test" } })
  // @ts-expect-error config overrides need a declared config
  complete.client.create({ name: "main" }, { config: { x: 1 } })
  // @ts-expect-error getById takes a typed agent ID, not its text
  complete.client.getById("00000001-0000-8000-8000-000000000001")

  const ephemeral = defineAgentClient({
    name: "Echo",
    id: { name: Schema.String },
    mode: "ephemeral",
    methods,
  })
  ephemeral.client.upsert({ name: "main" })
  // @ts-expect-error ephemeral clients cannot create a durable agent
  ephemeral.client.create({ name: "main" })
  // @ts-expect-error ephemeral clients have no agent ID to bind
  ephemeral.client.getById(id)

  const onlyMethods = defineAgentClient({ methods })
  Client.bind(id, onlyMethods)
  // @ts-expect-error binding-only definitions have no lifecycle factories
  onlyMethods.client.upsert({ name: "main" })

  // @ts-expect-error the name requires an id
  defineAgentClient({ name: "Echo", methods })
  // @ts-expect-error the id requires a name
  defineAgentClient({ id: { name: Schema.String }, methods })
  // @ts-expect-error config is not allowed on a method-only client
  defineAgentClient({ methods, config: undefined })
  // @ts-expect-error mode is not allowed on a method-only client
  defineAgentClient({ methods, mode: "durable" })
}

it("checks caller-definition shapes at compile time", () => {
  expect(typeof checkDefinitions).toBe("function")
})
