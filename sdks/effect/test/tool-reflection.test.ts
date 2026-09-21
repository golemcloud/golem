import { describe, expect, it, vi } from "vitest"
import { Effect, Schema } from "effect"
import { c, compileDefinition, toolDefinition } from "../src/internal/tool/model.js"
import { ToolType } from "../src/ToolReflection.js"
import { toolClientDefinition, ToolTransport } from "../src/Tool.js"
import { ToolClient } from "../src/host/ToolClient.js"
import { compile } from "../src/WitCodec.js"
import { t } from "../src/internal/schema-model/model.js"
import { schemaGraphToWit } from "../src/internal/schema-model/wit.js"
import { SchemaRef } from "../src/SchemaRef.js"

const definition = toolDefinition("effect-reflection").body((body) =>
  body.positional("name", Schema.String).returns(Schema.String),
)
const registered = {
  lookupName: "effect-reflection",
  definition: compileDefinition(definition).wire,
  implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
}

describe("native tool reflection", () => {
  it("treats a default-true negatable flag as present when set to false", () => {
    const definition = toolDefinition("negatable-reflection").body((body) =>
      body
        .flag("enabled", { default: true, negatable: true })
        .constraint(c.requiresAll(c.present("enabled"))),
    )
    const command = new ToolType({
      ...registered,
      lookupName: "negatable-reflection",
      definition: compileDefinition(definition).wire,
    }).client.command([])
    expect(command.validateJson({ enabled: false }).success).toBe(true)
    expect(command.validateJson({ enabled: true }).success).toBe(false)
  })

  it("matches ValueIs through an optional positional carrier", () => {
    const expected = Effect.runSync(
      compile(Schema.String).pipe(Effect.flatMap((codec) => codec.encode("needle"))),
    )
    const definition = toolDefinition("nested-value-is").body((body) =>
      body
        .positional("maybe", Schema.String, { required: false })
        .constraint(c.requiresAll(c.valueIs("maybe", expected))),
    )
    const command = new ToolType({
      ...registered,
      lookupName: "nested-value-is",
      definition: compileDefinition(definition).wire,
    }).client.command([])
    expect(command.validateJson({ maybe: "needle" }).success).toBe(true)
    expect(command.validateJson({ maybe: null }).success).toBe(false)
    expect(command.validateJson({ maybe: "other" }).success).toBe(false)
  })

  it("sends optional inputs with a graph that accepts both carriers", async () => {
    const optionalDefinition = toolDefinition("effect-optional-reflection").body((body) =>
      body.option("maybe", Schema.String).returns(Schema.String),
    )
    const compiled = compileDefinition(optionalDefinition)
    const optionalRegistered = {
      ...registered,
      lookupName: "effect-optional-reflection",
      definition: compiled.wire,
    }
    const codec = Effect.runSync(compile(Schema.String))
    const sent: Array<Parameters<ToolTransport["start"]>[2]> = []
    const transport = ToolTransport.of({
      start: (_tool, _path, input) => {
        sent.push(input)
        return Effect.succeed({
          result: Effect.succeed({
            result: {
              graph: codec.schemaGraph,
              value: Effect.runSync(codec.encode("ok") as Effect.Effect<any, any>),
            },
          }),
          cancel: Effect.void,
        })
      },
    })
    const host = ToolClient.of({
      getAllTools: () => [optionalRegistered],
      getTool: () => optionalRegistered,
      createStdin: vi.fn() as never,
      createStdinFromStream: vi.fn() as never,
      createStdout: vi.fn() as never,
      rpc: vi.fn() as never,
      createRpc: vi.fn() as never,
    })
    const command = new ToolType(optionalRegistered).client.command([])
    expect(new SchemaRef(compiled.bodies.get("")!.input.schemaGraph).toJsonSchema()).toEqual(
      command.inputSchema?.toJsonSchema(),
    )
    for (const maybe of [null, "supplied"]) {
      await expect(
        Effect.runPromise(
          command
            .invokeJson({ maybe })
            .pipe(
              Effect.provideService(ToolTransport, transport),
              Effect.provideService(ToolClient, host),
            ),
        ),
      ).resolves.toBe("ok")
      const value = sent.at(-1)!
      const wire = new SchemaRef(value.graph)
      expect(wire.validateValue(value.value).success).toBe(true)
      expect(wire.toJsonSchema()).toEqual(command.inputSchema?.toJsonSchema())
    }
    expect(sent).toHaveLength(2)
  })

  it("decodes a discovered command with a selected schema root", () => {
    const tool = new ToolType(registered)
    const command = tool.client.command([])
    expect(command.path).toEqual([])
    expect(command.validateJson({ name: "hello" }).success).toBe(true)
    const invalid = command.validateJson({ name: 4 })
    expect(invalid.success).toBe(false)
    if (!invalid.success) {
      expect(invalid.issues[0]?.phase).toBe("input")
      expect(invalid.issues[0]?.cause).toBeInstanceOf(Error)
      expect(invalid.issues[0]?.path).toEqual(["name"])
    }
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
      createRpc: vi.fn() as never,
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

  it("settles result and stdout and gives the result error precedence", async () => {
    const streaming = toolDefinition("settle-both").body((body) =>
      body.positional("name", Schema.String).output().returns(Schema.String),
    )
    const registration = {
      ...registered,
      lookupName: "settle-both",
      definition: compileDefinition(streaming).wire,
    }
    let stdoutSettled = false
    const transport = ToolTransport.of({
      start: () =>
        Effect.succeed({
          stdout: (async function* () {
            try {
              yield { tag: "err", val: { tag: "failed", val: "stdout failed" } } as const
            } finally {
              stdoutSettled = true
            }
          })(),
          result: Effect.fail({ tag: "denied", val: "result failed" } as const),
          cancel: Effect.void,
        }),
    })
    const host = ToolClient.of({
      getAllTools: () => [registration],
      getTool: () => registration,
      createStdin: vi.fn() as never,
      createStdinFromStream: vi.fn() as never,
      createStdout: vi.fn() as never,
      rpc: vi.fn() as never,
      createRpc: vi.fn() as never,
    })
    const failure = new ToolType(registration).client
      .command([])
      .startJson({ name: "hello" })
      .pipe(
        Effect.flatMap((started) => started.collect),
        Effect.provideService(ToolTransport, transport),
        Effect.provideService(ToolClient, host),
        Effect.scoped,
        Effect.flip,
      )

    await expect(Effect.runPromise(failure)).resolves.toMatchObject({
      tag: "rpc",
      error: { tag: "denied", val: "result failed" },
    })
    expect(stdoutSettled).toBe(true)
  })

  it("uses fallible reflected RPC creation for triggers and preserves its structured error", async () => {
    const createRpc = vi.fn(() => {
      throw { tag: "denied", val: "not granted" }
    })
    const rpc = vi.fn(() => {
      throw new Error("infallible constructor must not be used")
    })
    const host = ToolClient.of({
      getAllTools: () => [registered],
      getTool: () => registered,
      createStdin: vi.fn() as never,
      createStdinFromStream: vi.fn() as never,
      createStdout: vi.fn() as never,
      rpc: rpc as never,
      createRpc: createRpc as never,
    })
    const command = new ToolType(registered).client.command([])
    const failure = command
      .triggerJson({ name: "hello" })
      .pipe(Effect.provideService(ToolClient, host), Effect.flip)

    await expect(Effect.runPromise(failure)).resolves.toEqual({
      tag: "rpc",
      error: { tag: "denied", val: "not granted" },
    })
    const startFailure = command
      .startJson({ name: "hello" })
      .pipe(Effect.provideService(ToolClient, host), Effect.scoped, Effect.flip)
    await expect(Effect.runPromise(startFailure)).resolves.toEqual({
      tag: "rpc",
      error: { tag: "denied", val: "not granted" },
    })
    expect(createRpc).toHaveBeenCalledTimes(2)
    expect(createRpc).toHaveBeenCalledWith("effect-reflection")
    expect(rpc).not.toHaveBeenCalled()
  })

  it("rejects a remote result whose graph differs from the declaration", async () => {
    const codec = Effect.runSync(compile(Schema.String))
    const transport = ToolTransport.of({
      start: () =>
        Effect.succeed({
          result: Effect.succeed({
            result: {
              graph: schemaGraphToWit({ defs: new Map(), root: t.bool() }),
              value: Effect.runSync(codec.encode("ok") as Effect.Effect<any, any>),
            },
          }),
          cancel: Effect.void,
        }),
    })
    const host = ToolClient.of({
      getAllTools: () => [registered],
      getTool: () => registered,
      createStdin: vi.fn() as never,
      createStdinFromStream: vi.fn() as never,
      createStdout: vi.fn() as never,
      rpc: vi.fn() as never,
      createRpc: vi.fn() as never,
    })
    const call = new ToolType(registered).client
      .command([])
      .invokeJson({ name: "hello" })
      .pipe(
        Effect.provideService(ToolTransport, transport),
        Effect.provideService(ToolClient, host),
        Effect.flip,
      )
    await expect(Effect.runPromise(call)).resolves.toMatchObject({ phase: "output" })
  })

  it("constructs definition-owned and caller-defined typed clients", () => {
    const transport = ToolTransport.of({ start: () => Effect.die("unused") })
    expect(typeof definition.client({ transport })).toBe("function")
    expect(typeof toolClientDefinition(definition).client("another-tool", { transport })).toBe(
      "function",
    )
    expect(() => toolClientDefinition(definition).client()).toThrow("requires a target name")
  })
})
