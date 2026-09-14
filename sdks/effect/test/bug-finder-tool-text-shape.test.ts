import { Effect, Schema } from "effect"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { resetTools, toolDefinition } from "../src/Tool.js"
import { invokeRegistered } from "../src/internal/tool/runtime.js"
import { compile } from "../src/WitCodec.js"
import { Text } from "../src/WitTypes.js"

describe("tool input structural compatibility", () => {
  beforeEach(resetTools)

  it("rejects a rich-text value for a string field as invalid input without invoking the handler", async () => {
    const handler = vi.fn((_input: { readonly message: string }) => Effect.void)
    toolDefinition("text-compatible")
      .body((body) => body.positional("message", Schema.String))
      .implement({ textCompatible: handler })

    const caller = Effect.runSync(compile(Schema.Struct({ message: Text() })))
    await expect(
      invokeRegistered(
        "text-compatible",
        [],
        {
          graph: caller.schemaGraph,
          value: Effect.runSync(caller.encode({ message: "hello" })),
        },
        undefined,
        undefined,
        {},
      ),
    ).rejects.toMatchObject({ tag: "invalid-input" })

    expect(handler).not.toHaveBeenCalled()
  })
})
