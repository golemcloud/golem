import { readFileSync } from "node:fs"
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
import { __setParseAgentIdImpl } from "./mocks/golem-agent-host.js"
import { __setEnvironment } from "./mocks/wasi-cli-environment.js"
import { componentConfiguration } from "../build/component.mjs"
import config from "../vitest.config.js"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const sdk = resolve(root, "dist/src")
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
  const options = await componentConfiguration(rollup, {
    input: resolve(root, `test/fixtures/capabilities/${fixture}.mjs`),
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
    const { output } = await bundle.generate({ format: "cjs", inlineDynamicImports: true })
    const chunk = output.find((item): item is OutputChunk => item.type === "chunk")!
    const capabilities = output.find(
      (item) => item.type === "asset" && item.fileName === "capabilities.json",
    )!
    const modules = Object.entries(chunk.modules)
      .filter(([, info]) => info.renderedLength > 0)
      .map(([id]) => id.replace(sdk + "/", ""))
    const hosts = new Map<string, unknown>([["effect", EffectRuntime]])
    for (const id of chunk.imports) {
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
