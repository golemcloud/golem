import { test, expect } from "bun:test"
import { readFile } from "fs/promises"
import { join } from "path"
import { unified } from "unified"
import remarkParse from "remark-parse"
import remarkMdx from "remark-mdx"
import { visit } from "unist-util-visit"
import { typecheckPublicExample } from "./typecheck-public-example"

async function fences(version: string, file: string, language: string) {
  const source = await readFile(join(import.meta.dir, "../src/content", version, file), "utf8")
  const tree = unified().use(remarkParse).use(remarkMdx).parse(source)
  const blocks: string[] = []
  visit(tree, "code", node => {
    if (node.lang === language) blocks.push(node.value)
  })
  return blocks
}

// These checks consume complete public fences, not copies or SDK mocks. They
// check guest source types, not WASM compilation, host calls, or provider access.
for (const version of ["next", "v1.6"]) {
  for (const page of ["develop/ai.mdx", "develop/rpc.mdx"]) {
    test(`${version}/${page}: TypeScript and Effect examples typecheck against the in-tree SDKs`, async () => {
      const blocks = (await fences(version, page, "typescript")).filter(code =>
        page === "develop/rpc.mdx"
          ? code.includes("export const WeatherAgent = defineAgent(")
          : code.includes("function askProvider(")
      )
      expect(blocks).toHaveLength(2)
      for (const code of blocks) {
        expect(typecheckPublicExample(code)).toEqual([])
      }
    }, 60_000)
  }

  test(`${version}: SDK typechecking rejects removed APIs and incorrect RPC id types`, async () => {
    const blocks = await fences(version, "develop/rpc.mdx", "typescript")
    const plain = blocks.find(code => code.includes("from '@golemcloud/golem-ts-sdk'"))!
    const effect = blocks.find(code => code.includes('from "@golemcloud/effect-golem"'))!
    expect(plain).toContain('location: "London"')
    expect(effect).toContain("WeatherAgent.client.get(")
    const wrongId = plain.replace('location: "London"', "location: 123")
    const removedApi = effect.replace(
      "WeatherAgent.client.get(",
      "WeatherAgent.client.removedPublicApi("
    )
    expect(typecheckPublicExample(wrongId).join("\n")).toContain(
      "Type 'number' is not assignable to type 'string'"
    )
    expect(typecheckPublicExample(removedApi).join("\n")).toContain(
      "Property 'removedPublicApi' does not exist"
    )
  }, 60_000)

  test(`${version}: public record, tuple, list, map and flags REST examples parse to their documented wire shapes`, async () => {
    const blocks = await fences(version, "type-mapping.mdx", "json")
    // Select complete single-value examples. Some other fences intentionally show alternatives.
    const values = blocks.filter(code => !code.includes("\n")).map(code => JSON.parse(code))
    expect(values).toContainEqual({ a: "hello", b: 1234 })
    expect(values).toContainEqual([1234, "hello world", 103])
    expect(values).toContainEqual(["one", "two", "three"])
    expect(values).toContainEqual([
      ["key1", 100],
      ["key2", -2],
    ])
    expect(values).toContainEqual(["get", "put"])
  })
}
