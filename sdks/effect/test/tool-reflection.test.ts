import { describe, expect, it, vi } from "vitest"
import { Effect, Schema } from "effect"
import { compileDefinition, toolDefinition } from "../src/internal/tool/model.js"
import { ToolType } from "../src/ToolReflection.js"
import { toolClientDefinition, ToolTransport } from "../src/Tool.js"
import { ToolClient } from "../src/host/ToolClient.js"
import { compile } from "../src/WitCodec.js"

const definition = toolDefinition("effect-reflection").body((body) =>
  body.positional("name", Schema.String).returns(Schema.String),
)
const registered = {
  lookupName: "effect-reflection",
  definition: compileDefinition(definition).wire,
  implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
}

describe("native tool reflection", () => {
  it("decodes a discovered command with a selected schema root", () => {
    const tool = new ToolType(registered)
    const command = tool.client.command([])
    expect(command.path).toEqual([])
    expect(command.validateJson({ name: "hello" }).success).toBe(true)
    expect(command.validateJson({ name: 4 }).success).toBe(false)
    expect(command.result?.toJsonSchema()).toBeDefined()
  })

  it("returns a validated result and releases its scoped invocation", async () => {
    const codec = Effect.runSync(compile(Schema.String))
    const cancel = vi.fn()
    const transport = ToolTransport.of({
      start: () =>
        Effect.succeed({
          result: Effect.succeed({
            result: {
              graph: codec.schemaGraph,
              value: Effect.runSync(codec.encode("ok") as Effect.Effect<any, any>),
            },
          }),
          cancel: Effect.sync(cancel),
        }),
    })
    const host = ToolClient.of({
      getAllTools: () => [registered],
      getTool: () => registered,
      createStdin: vi.fn() as never,
      createStdinFromStream: vi.fn() as never,
      createStdout: vi.fn() as never,
      rpc: vi.fn() as never,
    })
    const call = new ToolType(registered).client
      .command([])
      .invokeJson({ name: "hello" })
      .pipe(
        Effect.provideService(ToolTransport, transport),
        Effect.provideService(ToolClient, host),
      )
    await expect(Effect.runPromise(call)).resolves.toBe("ok")
    expect(cancel).toHaveBeenCalledTimes(1)
  })

  it("constructs exact and partial typed clients from their definitions", () => {
    const transport = ToolTransport.of({ start: () => Effect.die("unused") })
    expect(typeof definition.client({ transport })).toBe("function")
    expect(typeof toolClientDefinition(definition).client("another-tool", { transport })).toBe(
      "function",
    )
    expect(() => toolClientDefinition(definition).client()).toThrow("requires a target name")
  })
})
