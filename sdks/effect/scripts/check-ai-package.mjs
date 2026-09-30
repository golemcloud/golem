import { readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { rollup } from "rollup"
import nodeResolve from "@rollup/plugin-node-resolve"
import { externalPackages } from "../integration-test/component-bundle-policy.mjs"

const packageDirectory = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const manifest = JSON.parse(readFileSync(resolve(packageDirectory, "package.json"), "utf8"))
const rootBundle = readFileSync(resolve(packageDirectory, manifest.exports["."].import), "utf8")

if (rootBundle.includes("effect-golem/ai") || rootBundle.includes("effect/unstable/ai")) {
  throw new Error("The root bundle includes the optional AI integration")
}
for (const dependencies of [manifest.dependencies, manifest.optionalDependencies]) {
  for (const name of Object.keys(dependencies ?? {})) {
    if (name.startsWith("@effect/ai-"))
      throw new Error(`Provider package must stay optional: ${name}`)
  }
}

const bundle = async (subpath) => {
  const build = await rollup({
    input: `virtual:${subpath}`,
    external: externalPackages,
    plugins: [
      {
        name: "ai-package-smoke-entry",
        resolveId(id) {
          if (id === `virtual:${subpath}`) return id
          if (id === "@golemcloud/effect-golem/ai") {
            return resolve(packageDirectory, manifest.exports["./ai"].import)
          }
        },
        load(id) {
          if (id === `virtual:${subpath}`) return `import ${JSON.stringify(subpath)}`
        },
      },
      nodeResolve(),
    ],
  })
  const generated = await build.generate({ format: "esm" })
  await build.close()
  return generated.output.map((output) => (output.type === "chunk" ? output.code : "")).join("\n")
}

const rootOnly = await bundle("@golemcloud/effect-golem")
if (!rootOnly.includes("@golemcloud/effect-golem")) {
  throw new Error("The representative root-only component did not keep the base SDK external")
}

const withAi = await bundle("@golemcloud/effect-golem/ai")
if (withAi.includes("@golemcloud/effect-golem/ai")) {
  throw new Error("The representative AI component externalized the optional AI entrypoint")
}

console.log("AI package boundary smoke test passed")
