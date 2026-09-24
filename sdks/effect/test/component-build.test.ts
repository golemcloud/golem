import { existsSync, readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { rollup, type OutputChunk } from "rollup"
import ts from "typescript"
import { nodeResolve } from "@rollup/plugin-node-resolve"
import { describe, expect, it, vi } from "vitest"
import * as EffectRuntime from "effect"
import { compile } from "../src/WitCodec.js"
import { schemaValueFromWit, schemaValueToWit } from "../src/internal/schema-model/wit.js"
import { v } from "../src/internal/schema-model/model.js"
import {
  __setParseAgentIdImpl,
  __setGetConfigValueImpl,
  __resetGetConfigValueImpl,
  __setRpcResponder,
  __getRecordedRpcCalls,
  __resetRpc,
} from "./mocks/golem-agent-host.js"
import { __setEnvironment } from "./mocks/wasi-cli-environment.js"
import { componentConfiguration } from "../build/component.mjs"
import config from "../vitest.config.js"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const sdk = resolve(root, "dist/src")
const httpContract = resolve(root, "../http-contract")
const exports = [
  "golemAgent200Guest",
  "golemTool010Guest",
  "loadSnapshot",
  "saveSnapshot",
  "toolMiddlewareGuest",
]

// Load the compiler output in memory so unit tests need no prebuilt dist files.
const sourcePlugin = {
  name: "sdk-test-sources",
  resolveId(id: string, importer?: string) {
    if (importer?.startsWith(sdk) && id.startsWith(".")) return resolve(dirname(importer), id)
    return null
  },
  load(id: string) {
    if (
      (id.startsWith(resolve(root, "test/fixtures/") + "/") || id.startsWith(httpContract + "/")) &&
      id.endsWith(".ts")
    )
      return ts.transpileModule(readFileSync(id, "utf8"), {
        compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
      }).outputText
    if (!id.startsWith(sdk + "/")) return null
    return ts.transpileModule(
      readFileSync(id.replace(sdk, resolve(root, "src")).replace(/\.js$/, ".ts"), "utf8"),
      {
        compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
      },
    ).outputText
  },
}

const build = async (fixture: string) => {
  const source = resolve(root, `test/fixtures/capabilities/${fixture}`)
  const options = await componentConfiguration(rollup, {
    input: existsSync(`${source}.ts`) ? `${source}.ts` : `${source}.mjs`,
    external: (id) =>
      id === "effect" || id === "node:sqlite" || id.startsWith("golem:") || id.startsWith("wasi:"),
    plugins: [sourcePlugin, nodeResolve()],
    onwarn: (warning) => {
      if (warning.code !== "CIRCULAR_DEPENDENCY" && warning.code !== "EMPTY_BUNDLE")
        throw new Error(warning.message)
    },
  })
  const bundle = await rollup(options)
  try {
    const { output } = await bundle.generate({
      format: "cjs",
      inlineDynamicImports: true,
      dynamicImportInCjs: false,
    })
    const chunk = output.find((item): item is OutputChunk => item.type === "chunk")!
    const capabilities = output.find(
      (item) => item.type === "asset" && item.fileName === "capabilities.json",
    )!
    const modules = Object.entries(chunk.modules)
      .filter(([, info]) => info.renderedLength > 0)
      .map(([id]) => id.replace(sdk + "/", ""))
    const hosts = new Map<string, unknown>([["effect", EffectRuntime]])
    for (const id of new Set([...chunk.imports, ...chunk.dynamicImports])) {
      if (id === "effect") continue
      const alias = (config.resolve!.alias as Array<{ find: string; replacement: string }>).find(
        (a) => a.find === id,
      )
      if (!alias) throw new Error(`No host fake for ${id}`)
      hosts.set(id, await import(alias.replacement))
    }
    const instantiate = () => {
      const runtime: any = {}
      new Function("require", "exports", chunk.code)((id: string) => hosts.get(id), runtime)
      return runtime
    }
    return {
      runtime: instantiate(),
      instantiate,
      chunk,
      modules,
      capabilities: JSON.parse(String("source" in capabilities && capabilities.source)),
    }
  } finally {
    await bundle.close()
  }
}

describe("capability-sensitive component exports", () => {
  it("does not rewrite a shadowed local DSL function", async () => {
    const { runtime } = await build("shadowed-dsl")
    expect(runtime.golemAgent200Guest).toBeDefined()
  })

  it("uses concrete nested codecs and transformed stream items without retaining the model", async () => {
    const { runtime, modules } = await build("concrete")
    expect(
      modules.filter((id) =>
        /internal\/schema-model\/(model|validation|builder|wit)\.js$/.test(id),
      ),
    ).toEqual([])
    const principal = { tag: "anonymous" }
    const payload = v.record([
      v.variant(1, v.record([v.list([v.f64(3), v.f64(11)])])),
      v.option(v.string("29")),
    ])
    const input = schemaValueToWit(v.record([payload]))
    await runtime.golemAgent200Guest.initialize(
      "ConcreteAgent",
      schemaValueToWit(v.record([])),
      principal,
    )
    const result = await runtime.golemAgent200Guest.invoke("echo", input, principal)
    expect(schemaValueFromWit(result)).toEqual(v.ok(payload))
    for (const optional of [
      v.record([v.option(undefined), v.option(undefined), v.option(undefined)]),
      v.record([
        v.option(v.option(v.string("present"))),
        v.option(v.string("43")),
        v.option(v.option(v.f64(17))),
      ]),
      v.record([v.option(undefined), v.option(undefined), v.option(v.option(undefined))]),
    ]) {
      expect(
        schemaValueFromWit(
          await runtime.golemAgent200Guest.invoke(
            "optionalFields",
            schemaValueToWit(v.record([optional])),
            principal,
          ),
        ),
      ).toEqual(optional)
    }
    const stringCodec = EffectRuntime.Effect.runSync(compile(EffectRuntime.Schema.String))
    let response: EffectRuntime.Effect.Effect<any, unknown> = EffectRuntime.Effect.succeed({
      result: { graph: stringCodec.schemaGraph, value: schemaValueToWit(v.string("71")) },
    })
    const cancel = vi.fn()
    vi.stubGlobal("__concreteToolStart", (name: string, path: string[], input: any) => {
      expect(name).toBe("remote-tool")
      expect(path).toEqual(["do-work"])
      expect(schemaValueFromWit(input.value)).toEqual(v.record([v.string("23")]))
      return EffectRuntime.Effect.succeed({
        result: response,
        cancel: EffectRuntime.Effect.sync(cancel),
      })
    })
    const invokeTool = async () =>
      schemaValueFromWit(
        await runtime.golemAgent200Guest.invoke("tool", schemaValueToWit(v.record([])), principal),
      )
    try {
      expect(await invokeTool()).toEqual(v.ok(v.f64(71)))
      response = EffectRuntime.Effect.succeed({
        result: { graph: stringCodec.schemaGraph, value: schemaValueToWit(v.f64(71)) },
      })
      expect(await invokeTool()).toEqual(v.err(v.string("result")))
      response = EffectRuntime.Effect.fail({
        tag: "custom-error",
        val: {
          name: "denied",
          payload: {
            graph: stringCodec.schemaGraph,
            value: schemaValueToWit(v.string("blocked")),
          },
        },
      })
      expect(await invokeTool()).toEqual(v.err(v.string("denied:blocked")))
      expect(cancel).toHaveBeenCalledTimes(3)
    } finally {
      vi.unstubAllGlobals()
    }
    const text = v.variant(0, { tag: "text", text: "hello", language: "en" })
    expect(
      schemaValueFromWit(
        await runtime.golemAgent200Guest.invoke(
          "text",
          schemaValueToWit(v.record([text])),
          principal,
        ),
      ),
    ).toEqual(text)
    await expect(
      runtime.golemAgent200Guest.invoke(
        "text",
        schemaValueToWit(
          v.record([v.variant(0, { tag: "text", text: "bonjour", language: "fr" })]),
        ),
        principal,
      ),
    ).rejects.toBeDefined()
    const content = v.list([
      v.variant(2, v.string("37")),
      v.variant(0, text),
      v.variant(
        1,
        v.variant(0, { tag: "binary", bytes: new Uint8Array([9, 17]), mimeType: "image/png" }),
      ),
    ])
    expect(
      schemaValueFromWit(
        await runtime.golemAgent200Guest.invoke(
          "content",
          schemaValueToWit(v.record([content])),
          principal,
        ),
      ),
    ).toEqual(v.list([v.string("count:38"), v.string("text:5"), v.string("binary:2")]))
    await expect(
      runtime.golemAgent200Guest.invoke(
        "content",
        schemaValueToWit(
          v.record([
            v.list([
              v.variant(
                1,
                v.variant(0, {
                  tag: "binary",
                  bytes: new Uint8Array([3]),
                  mimeType: "text/plain",
                }),
              ),
            ]),
          ]),
        ),
        principal,
      ),
    ).rejects.toBeDefined()
    __setRpcResponder((call) => {
      expect(call.agentTypeName).toBe("Remote")
      expect(call.methodName).toBe("echo")
      expect(schemaValueFromWit(call.input)).toEqual(v.record([v.string("17")]))
      return { tag: "ok", val: schemaValueToWit(v.ok(v.string("41"))) }
    })
    try {
      expect(
        schemaValueFromWit(
          await runtime.golemAgent200Guest.invoke(
            "remote",
            schemaValueToWit(v.record([v.f64(17)])),
            principal,
          ),
        ),
      ).toEqual(v.ok(v.f64(41)))
      expect(schemaValueFromWit(__getRecordedRpcCalls()[0]!.constructorValue)).toEqual(
        v.record([v.string("3")]),
      )
    } finally {
      __resetRpc()
    }
    const rich = v.record([
      v.fixedList([v.string("first"), v.string("last")]),
      v.list([v.u16(513), v.u16(29)]),
      v.map([{ key: v.string("answer"), value: v.string("43") }]),
      v.list([v.tuple([v.string("count"), v.f64(7)])]),
      v.text("rich text"),
      v.binary(new Uint8Array([9, 255])),
      v.flags([true, false, true]),
      v.duration(1500000001n),
      v.variant(2, v.record([v.record([v.record([v.u64(17n), v.u64(31n)])])])),
    ])
    expect(
      schemaValueFromWit(
        await runtime.golemAgent200Guest.invoke(
          "rich",
          schemaValueToWit(v.record([rich])),
          principal,
        ),
      ),
    ).toEqual(rich)
    const wrongListTag = schemaValueToWit(v.record([rich]))
    const fixed = wrongListTag.valueNodes.findIndex((n) => n.tag === "fixed-list-value")
    wrongListTag.valueNodes[fixed] = { tag: "list-value", val: [0, 1] }
    await expect(
      runtime.golemAgent200Guest.invoke("rich", wrongListTag, principal),
    ).rejects.toBeDefined()
    const failure = await runtime.golemAgent200Guest.invoke(
      "echo",
      schemaValueToWit(
        v.record([v.record([v.variant(0, v.record([v.string("denied")])), v.option(undefined)])]),
      ),
      principal,
    )
    expect(schemaValueFromWit(failure)).toEqual(v.err(v.string("denied")))

    const closed = vi.fn(async () => ({ done: true, value: undefined }))
    const values = ["17", "41"]
    const stream = {
      source: {
        [Symbol.asyncIterator]: () => ({
          next: async () =>
            values.length
              ? { done: false, value: schemaValueToWit(v.string(values.shift()!)) }
              : { done: true, value: undefined },
          return: closed,
        }),
      },
    }
    const collected = await runtime.golemAgent200Guest.invoke(
      "collect",
      {
        root: 1,
        valueNodes: [
          { tag: "stream-value", val: stream },
          { tag: "record-value", val: [0] },
        ],
      },
      principal,
    )
    expect(schemaValueFromWit(collected)).toEqual(v.list([v.f64(17), v.f64(41)]))
    expect(closed).not.toHaveBeenCalled()
    await expect(
      runtime.golemAgent200Guest.invoke(
        "echo",
        schemaValueToWit(v.record([v.record([v.variant(0), v.option(undefined)])])),
        principal,
      ),
    ).rejects.toBeDefined()
    const reads: string[] = []
    __setGetConfigValueImpl((path, graph) => {
      reads.push(path.join("/"))
      if (path[0] === "nested") return schemaValueToWit(v.string("23"))
      expect(graph.typeNodes[graph.root].body.tag).toBe("secret-type")
      return { root: 0, valueNodes: [{ tag: "secret-value", val: {} }] }
    })
    try {
      const configured = await runtime.golemAgent200Guest.invoke(
        "configured",
        schemaValueToWit(v.record([])),
        principal,
      )
      expect(schemaValueFromWit(configured)).toEqual(v.record([v.f64(46), v.bool(true)]))
      expect(reads).toEqual(["nested/number", "token", "token"])
    } finally {
      __resetGetConfigValueImpl()
    }
  }, 30000)

  it.each([
    ["empty", false, false, false],
    ["tool-only", false, true, false],
    ["agent-only", true, false, false],
    ["middleware-only", false, false, true],
    ["mixed", true, true, true],
  ] as const)(
    "%s keeps the ABI and only the selected runtime",
    async (fixture, agents, tools, middleware) => {
      const { runtime, instantiate, chunk, modules, capabilities } = await build(fixture)
      expect(chunk.exports).toEqual(exports)
      expect(capabilities).toEqual({ agents, tools, middleware })
      expect(modules.includes("internal/agent.js")).toBe(agents)
      expect(modules.includes("internal/snapshotEnvelope.js")).toBe(agents)
      expect(modules.includes("Snapshot.js")).toBe(agents)
      expect(modules.includes("internal/tool/runtime.js")).toBe(tools)
      expect(modules.includes("internal/tool/registry.js")).toBe(tools)
      expect(modules.includes("internal/tool/middleware.js")).toBe(middleware)
      expect(modules.includes("Reflection.js")).toBe(false)
      expect(modules.includes("DynamicClient.js")).toBe(false)
      expect(modules.includes("SchemaRef.js")).toBe(false)
      expect(modules.some((id) => id.startsWith("internal/reflection/"))).toBe(false)
      if (!middleware) {
        expect(
          modules.filter((id) =>
            /internal\/schema-model\/(model|validation|builder|wit)\.js$/.test(id),
          ),
        ).toEqual([])
        expect(chunk.code).not.toMatch(
          /decodeCanonicalInputRecord|validateSchemaGraph|schemaValueConforms/,
        )
      }
      if (!agents)
        expect(modules.some((id) => /^(Sqlite|Postgres|Mysql|Ignite)\//.test(id))).toBe(false)
      expect(runtime.golemAgent200Guest.discoverAgentTypes().map((a: any) => a.typeName)).toEqual(
        agents ? ["CapabilityCounter"] : [],
      )
      expect(
        runtime.golemTool010Guest.discoverTools().map((t: any) => t.commands.nodes[0].name),
      ).toEqual(tools ? ["double"] : [])
      expect(runtime.toolMiddlewareGuest.discoverToolMiddlewares().map((m: any) => m.name)).toEqual(
        middleware ? ["passthrough"] : [],
      )
      if (!agents) {
        await expect(runtime.golemAgent200Guest.initialize("Absent")).rejects.toThrow(
          "unknown agent",
        )
        await expect(runtime.saveSnapshot.save()).rejects.toThrow("not initialized")
        await expect(runtime.loadSnapshot.load()).rejects.toThrow("no agent")
      } else {
        const input = schemaValueToWit(v.record([v.f64(17)]))
        await runtime.golemAgent200Guest.initialize("CapabilityCounter", input, {
          tag: "anonymous",
        })
        expect(
          schemaValueFromWit(
            await runtime.golemAgent200Guest.invoke("get", schemaValueToWit(v.record([])), {
              tag: "anonymous",
            }),
          ),
        ).toEqual(v.f64(17))
        const snapshot = await runtime.saveSnapshot.save()
        __setEnvironment([["GOLEM_AGENT_ID", "CapabilityCounter(17)"]])
        __setParseAgentIdImpl(() => ["CapabilityCounter", { value: input }, undefined])
        const restored = instantiate()
        await restored.loadSnapshot.load(snapshot)
        expect(
          schemaValueFromWit(
            await restored.golemAgent200Guest.invoke("get", schemaValueToWit(v.record([])), {
              tag: "anonymous",
            }),
          ),
        ).toEqual(v.f64(17))
      }
      if (tools) {
        const codec = EffectRuntime.Effect.runSync(
          compile(EffectRuntime.Schema.Struct({ value: EffectRuntime.Schema.Number })),
        )
        const value = EffectRuntime.Effect.runSync(codec.encode({ value: 19 }))
        const result = await runtime.golemTool010Guest.invoke(
          "double",
          [],
          { graph: codec.schemaGraph, value },
          undefined,
          undefined,
          { tag: "anonymous" },
        )
        expect(schemaValueFromWit(result.result.value)).toEqual(v.f64(38))
        await expect(
          runtime.golemTool010Guest.invoke(
            "double",
            [],
            { graph: { typeNodes: [], defs: [], root: 99 }, value },
            undefined,
            undefined,
            { tag: "anonymous" },
          ),
        ).rejects.toMatchObject({ tag: "invalid-input" })
        await expect(
          runtime.golemTool010Guest.invoke(
            "double",
            [],
            {
              graph: { typeNodes: [], defs: [], root: 99 },
              value: { valueNodes: [{ tag: "record-value", val: [] }], root: 0 },
            },
            undefined,
            undefined,
            { tag: "anonymous" },
          ),
        ).rejects.toMatchObject({ tag: "invalid-input" })
      }
      if (!tools)
        await expect(runtime.golemTool010Guest.invoke("missing")).rejects.toEqual({
          tag: "invalid-tool-name",
          val: "missing",
        })
      if (middleware) {
        const codec = EffectRuntime.Effect.runSync(compile(EffectRuntime.Schema.Struct({})))
        const input = {
          graph: codec.schemaGraph,
          value: EffectRuntime.Effect.runSync(codec.encode({})),
        }
        const invoke = vi.fn(() => [
          { get: async () => input, cancel() {}, [Symbol.dispose]() {} },
          undefined,
        ])
        const result = await runtime.toolMiddlewareGuest.invokeToolMiddleware(
          "passthrough",
          "target",
          {},
          input,
          ["nested"],
          input,
          undefined,
          undefined,
          { tag: "anonymous" },
          { invoke },
        )
        expect(invoke).toHaveBeenCalledOnce()
        expect(invoke.mock.calls[0]).toEqual([["nested"], input, undefined])
        expect(result).toEqual({ result: input })
      } else
        await expect(runtime.toolMiddlewareGuest.invokeToolMiddleware("missing")).rejects.toEqual({
          tag: "invalid-tool-name",
          val: "missing",
        })
      if (fixture === "empty") expect(chunk.imports).toEqual([])
    },
    30000,
  )
})
