import { describe, expect, it } from "@effect/vitest"
import { Effect } from "effect"
import { Reflection } from "../src/index.js"

const dynamicSearch = Effect.scoped(
  Effect.gen(function* () {
    const type = yield* Reflection.getAgentType("SearchAgent")
    const method = type?.method("search")
    if (!type || type.mode !== "durable" || !method)
      return yield* Effect.fail("SearchAgent.search is unavailable")

    const input = method.input.packJson({ query: "golem", cursor: null })
    const inputCheck = method.input.validateValue(input)
    if (!inputCheck.success) return yield* Effect.fail(inputCheck.issues)

    const identity = yield* type.agentId({ tenant: "docs" })
    const dynamic = yield* identity.dynamicClient()
    const result = yield* dynamic
      .method(method.name)
      .invoke(input)
      .pipe(
        Effect.catch((error) =>
          Effect.logError("dynamic search failed", error).pipe(Effect.andThen(Effect.fail(error))),
        ),
      )
    if (!method.output || result.value === undefined)
      return yield* Effect.fail("search returned an unexpected unit result")
    const outputCheck = method.output.validateValue(result.value)
    if (!outputCheck.success) return yield* Effect.fail(outputCheck.issues)
    return method.output.unpackJson(result.value)
  }),
)

describe("discovery-to-dynamic guide", () => {
  it("constructs its scoped recipe without contacting the host", () => {
    expect(dynamicSearch).toBeDefined()
  })
})
