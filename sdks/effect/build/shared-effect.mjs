import { existsSync } from "node:fs"
import { createRequire } from "node:module"
import path from "node:path"
import { pathToFileURL } from "node:url"

export function sharedEffectRuntime(input) {
  const effectDistDir = path.join(
    path.dirname(createRequire(input).resolve("effect/package.json")),
    "dist",
  )
  const prefix = "\0golem-effect-root-facade:"
  const redacted = "\0golem-effect-redacted-facade"
  const stableModule = (source, importer) => {
    const subpath = /^effect\/([A-Za-z_$][A-Za-z0-9_$]*)$/.exec(source)
    if (subpath) {
      const name = subpath[1]
      return name !== "index" && existsSync(path.join(effectDistDir, `${name}.js`))
        ? name
        : undefined
    }
    if (!importer || !source.startsWith(".") || importer.startsWith("\0")) return undefined
    const relative = path.relative(effectDistDir, path.resolve(path.dirname(importer), source))
    return !relative.includes(path.sep) && relative.endsWith(".js")
      ? path.basename(relative, ".js")
      : undefined
  }
  return {
    name: "golem-shared-effect-runtime",
    resolveId(source, importer) {
      const name = stableModule(source, importer)
      if (name) return { id: prefix + name, moduleSideEffects: false }
      if (
        importer &&
        source.startsWith(".") &&
        !importer.startsWith("\0") &&
        path.resolve(path.dirname(importer), source) ===
          path.join(effectDistDir, "internal/redacted.js")
      )
        return { id: redacted, moduleSideEffects: false }
      return null
    },
    async load(id) {
      if (id === redacted)
        return `
        import { Redacted } from "effect";
        export const value = Redacted.value;
        export const stringOrRedacted = (input) =>
          typeof input === "string" ? input : Redacted.value(input);
      `
      if (!id.startsWith(prefix)) return null
      const name = id.slice(prefix.length)
      const module = await import(pathToFileURL(path.join(effectDistDir, `${name}.js`)))
      return [
        `import { ${name} as sharedModule } from "effect";`,
        ...Object.keys(module)
          .filter((key) => key !== "default" && /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(key))
          .map(
            (key, index) =>
              `const e${index} = /* @__PURE__ */ (() => sharedModule.${key})(); export { e${index} as ${key} };`,
          ),
      ].join("\n")
    },
  }
}
