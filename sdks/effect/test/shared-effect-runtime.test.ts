import { nodeResolve } from "@rollup/plugin-node-resolve"
import { describe, expect, it } from "vitest"
import { rollup, type OutputChunk } from "rollup"
import { sharedEffectRuntime } from "../build/shared-effect.mjs"

describe("shared Effect runtime", () => {
  it("routes an effect/http leaf module through the shared effect/http facade", async () => {
    const entry = "\0stable-http-leaf-entry"
    const bundle = await rollup({
      input: entry,
      external: (id) => id === "effect/http",
      plugins: [
        sharedEffectRuntime(import.meta.url),
        {
          name: "stable-http-leaf-entry",
          resolveId: (id) => (id === entry ? entry : null),
          load: (id) =>
            id === entry
              ? 'export { make, route, serve, toHttpEffect, toWebHandler } from "effect/http/HttpRouter"'
              : null,
        },
        nodeResolve(),
      ],
    })

    try {
      const { output } = await bundle.generate({ format: "esm" })
      const chunk = output.find((item): item is OutputChunk => item.type === "chunk")!
      expect(chunk.imports).toEqual(["effect/http"])
      expect(Object.keys(chunk.modules).some((id) => id.includes("/effect/dist/http/"))).toBe(false)
    } finally {
      await bundle.close()
    }
  })

  it("routes an effect/http-api leaf module through the shared Effect runtime", async () => {
    const entry = "\0stable-http-api-leaf-entry"
    const bundle = await rollup({
      input: entry,
      external: (id) => id === "effect",
      plugins: [
        sharedEffectRuntime(import.meta.url),
        {
          name: "stable-http-api-leaf-entry",
          resolveId: (id) => (id === entry ? entry : null),
          load: (id) => (id === entry ? 'export { make } from "effect/http-api/HttpApi"' : null),
        },
        nodeResolve(),
      ],
    })

    try {
      const { output } = await bundle.generate({ format: "esm" })
      const chunk = output.find((item): item is OutputChunk => item.type === "chunk")!
      expect(chunk.imports).toEqual(["effect"])
      expect(Object.keys(chunk.modules).some((id) => id.includes("/effect/dist/http-api/"))).toBe(
        false,
      )
    } finally {
      await bundle.close()
    }
  })
})
