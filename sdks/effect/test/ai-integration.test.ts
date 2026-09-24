import { describe, expect, it, vi } from "vitest"
import { Effect, Schema, Stream } from "effect"
import { LanguageModel } from "effect/unstable/ai"
import { command, reflectedToolkit, typedToolkit } from "../src/Ai.js"
import { ToolClient } from "../src/host/ToolClient.js"
import { compileDefinition, toolDefinition } from "../src/internal/tool/model.js"
import { ToolTransport, type ToolTransport as ToolTransportService } from "../src/Tool.js"
import { ToolType } from "../src/ToolReflection.js"
import { compile } from "../src/WitCodec.js"

const host = ToolClient.of({
  getAllTools: vi.fn(),
  getTool: vi.fn(),
  createStdin: vi.fn() as never,
  createStdinFromStream: vi.fn() as never,
  createStdout: vi.fn() as never,
  rpc: vi.fn() as never,
  createRpc: vi.fn() as never,
})

const fakeModel = (toolName: string, parameters: Record<string, unknown>) =>
  LanguageModel.make({
    generateText: (options) =>
      Effect.sync(() => {
        expect(options.tools.map((tool) => tool.name)).toContain(toolName)
        return [
          {
            type: "tool-call" as const,
            id: `call-${toolName}`,
            name: toolName,
            params: parameters,
          },
        ]
      }),
    streamText: () => Stream.empty,
  })

describe("Effect AI language model integration", () => {
  it("publishes matching parameter schemas for typed and reflected commands", async () => {
    const definition = toolDefinition("schema-parity").body((body) =>
      body
        .positional("required-value", Schema.String)
        .option("optional-value", Schema.Number)
        .flag("verbose"),
    )
    const transport: ToolTransportService = {
      start: () => Effect.die("not invoked"),
    }
    const typed = await Effect.runPromise(typedToolkit(definition, [command()], { transport }))
    const reflected = new ToolType({
      lookupName: definition.name,
      definition: compileDefinition(definition).wire,
      implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
    })
    const discovered = await Effect.runPromise(
      reflectedToolkit(reflected, [command()]).pipe(Effect.provideService(ToolClient, host)),
    )

    const discoveredSchema = discovered.tools["schema-parity"] as unknown as {
      readonly jsonSchema: unknown
    }
    const typedSchema = typed.tools["schema-parity"] as unknown as {
      readonly jsonSchema: unknown
    }
    expect(discoveredSchema.jsonSchema).toEqual(typedSchema.jsonSchema)
  })

  it("evaluates typed approval against normalized command defaults", async () => {
    const definition = toolDefinition("typed-approval").body((body) =>
      body.option("mode", Schema.String, { default: "destructive" }),
    )
    const start = vi.fn<ToolTransportService["start"]>(() =>
      Effect.succeed({ result: Effect.succeed({}), cancel: Effect.void }),
    )
    const approval = vi.fn(
      (parameters: Readonly<Record<string, unknown>>) => parameters.mode === "destructive",
    )
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command([], { needsApproval: approval })], {
        transport: { start },
      }),
    )
    const model = await Effect.runPromise(fakeModel("typed-approval", {}))

    const response = await Effect.runPromise(
      LanguageModel.generateText({ prompt: "Run the tool", toolkit }).pipe(
        Effect.provideService(LanguageModel.LanguageModel, model),
      ),
    )

    expect(approval).toHaveBeenCalledWith(
      { mode: "destructive" },
      expect.objectContaining({ toolCallId: "call-typed-approval" }),
    )
    expect(response.content.some((part) => part.type === "tool-approval-request")).toBe(true)
    expect(response.toolResults).toHaveLength(0)
    expect(start).not.toHaveBeenCalled()
  })

  it("evaluates reflected approval against normalized command defaults", async () => {
    const definition = toolDefinition("reflected-approval").body((body) =>
      body.option("mode", Schema.String, { default: "destructive" }),
    )
    const reflected = new ToolType({
      lookupName: definition.name,
      definition: compileDefinition(definition).wire,
      implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
    })
    const start = vi.fn<ToolTransportService["start"]>(() =>
      Effect.succeed({ result: Effect.succeed({}), cancel: Effect.void }),
    )
    const approval = vi.fn(
      (parameters: Readonly<Record<string, unknown>>) => parameters.mode === "destructive",
    )
    const toolkit = await Effect.runPromise(
      reflectedToolkit(reflected, [command([], { needsApproval: approval })]).pipe(
        Effect.provideService(ToolClient, host),
      ),
    )
    const model = await Effect.runPromise(fakeModel("reflected-approval", {}))

    const response = await Effect.runPromise(
      LanguageModel.generateText({ prompt: "Run the tool", toolkit }).pipe(
        Effect.provideService(LanguageModel.LanguageModel, model),
        Effect.provideService(ToolTransport, { start }),
        Effect.provideService(ToolClient, host),
      ),
    )

    expect(approval).toHaveBeenCalledWith(
      { mode: "destructive" },
      expect.objectContaining({ toolCallId: "call-reflected-approval" }),
    )
    expect(response.content.some((part) => part.type === "tool-approval-request")).toBe(true)
    expect(response.toolResults).toHaveLength(0)
    expect(start).not.toHaveBeenCalled()
  })

  it("executes a typed Golem tool from a provider tool call without a provider package", async () => {
    const definition = toolDefinition("calculator").body((body) =>
      body
        .positional("left", Schema.Number)
        .positional("right", Schema.Number)
        .returns(Schema.Number),
    )
    const input = Effect.runSync(
      compile(Schema.Struct({ left: Schema.Number, right: Schema.Number })),
    )
    const output = Effect.runSync(compile(Schema.Number))
    const start = vi.fn<ToolTransportService["start"]>((_tool, _path, encoded) =>
      Effect.gen(function* () {
        const { left, right } = yield* input.decode(encoded.value)
        return {
          result: Effect.succeed({
            result: {
              graph: output.schemaGraph,
              value: yield* output.encode(left + right),
            },
          }),
          cancel: Effect.void,
        }
      }),
    )
    const toolkit = await Effect.runPromise(
      typedToolkit(definition, [command()], { transport: { start } }),
    )
    const model = await Effect.runPromise(fakeModel("calculator", { left: 20, right: 22 }))

    const response = await Effect.runPromise(
      LanguageModel.generateText({ prompt: "Add the numbers", toolkit }).pipe(
        Effect.provideService(LanguageModel.LanguageModel, model),
      ),
    )

    expect(response.toolResults).toHaveLength(1)
    expect(response.toolResults[0]).toMatchObject({
      name: "calculator",
      isFailure: false,
      result: { status: "success", result: 42 },
    })
    expect(start).toHaveBeenCalledOnce()
  })

  it("executes a reflected Golem tool from the same provider-independent flow", async () => {
    const definition = toolDefinition("reflected-calculator").body((body) =>
      body.positional("value", Schema.Number).returns(Schema.Number),
    )
    const reflected = new ToolType({
      lookupName: definition.name,
      definition: compileDefinition(definition).wire,
      implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
    })
    const commandDefinition = reflected.client.command([])
    const output = Effect.runSync(compile(Schema.Number))
    const start = vi.fn<ToolTransportService["start"]>((_tool, _path, encoded) =>
      Effect.succeed({
        result: Effect.succeed({
          result: {
            graph: output.schemaGraph,
            value: Effect.runSync(
              output.encode(
                (commandDefinition.inputSchema!.unpackJson(encoded.value) as { value: number })
                  .value * 2,
              ),
            ),
          },
        }),
        cancel: Effect.void,
      }),
    )
    const toolkit = await Effect.runPromise(
      reflectedToolkit(reflected, [command()]).pipe(Effect.provideService(ToolClient, host)),
    )
    const model = await Effect.runPromise(fakeModel("reflected-calculator", { value: 21 }))

    const response = await Effect.runPromise(
      LanguageModel.generateText({ prompt: "Double the number", toolkit }).pipe(
        Effect.provideService(LanguageModel.LanguageModel, model),
        Effect.provideService(ToolTransport, { start }),
        Effect.provideService(ToolClient, host),
      ),
    )

    expect(response.toolResults).toHaveLength(1)
    expect(response.toolResults[0]).toMatchObject({
      name: "reflected-calculator",
      isFailure: false,
      result: { status: "success", result: 42 },
    })
    expect(start).toHaveBeenCalledOnce()
  })
})
