#!/usr/bin/env node
/**
 * Generates the Rust wrapper crate that embeds the bundled effect-golem
 * runtime into a QuickJS-backed WASM component, leaving a `user` slot to
 * be filled in later via `wasm-rquickjs inject-js`.
 *
 * Mirrors the corresponding script in golemcloud/golem's
 * sdks/ts/packages/golem-ts-sdk.
 */
import { spawnSync } from "node:child_process"
import { fileURLToPath } from "node:url"
import { dirname, resolve } from "node:path"
import { existsSync, rmSync, readFileSync, writeFileSync } from "node:fs"

const here = dirname(fileURLToPath(import.meta.url))
const root = resolve(here, "..")
const wit = resolve(root, "wit")
const output = resolve(root, "agent-template")
const sdkBundle = resolve(root, "dist/index.mjs")
const effectBundle = resolve(root, "dist/effect.mjs")

for (const f of [sdkBundle, effectBundle]) {
  if (!existsSync(f)) {
    console.error(`error: ${f} does not exist. Run "npm run build:bundle" first.`)
    process.exit(1)
  }
}

if (existsSync(output)) {
  rmSync(output, { recursive: true, force: true })
}

const result = spawnSync(
  "wasm-rquickjs",
  [
    "generate-wrapper-crate",
    "--wit",
    wit,
    "--output",
    output,
    "--world",
    "agent-guest",
    "--js-modules",
    `effect-golem=${sdkBundle}`,
    "--js-modules",
    `effect=${effectBundle}`,
    "--js-modules",
    "user=@slot",
  ],
  { stdio: "inherit", cwd: root },
)

if (result.status !== 0) {
  process.exit(result.status ?? 1)
}

// Workaround: the generated `JS_ADDITIONAL_MODULES` Vec expects each
// closure to return `String`, but for static (non-`@slot`) modules
// wasm-rquickjs emits `include_str!(...)` which yields `&'static str`.
// Coerce them with `.to_string()` so cargo will compile the crate.
const libRsPath = resolve(output, "src/lib.rs")
const original = readFileSync(libRsPath, "utf-8")
const patched = original.replace(
  /Box::new\(\|\|\s*\{\s*include_str!\("([^"]+)"\)\s*\}\)/g,
  'Box::new(|| { include_str!("$1").to_string() })',
)
if (patched !== original) {
  writeFileSync(libRsPath, patched, "utf-8")
  console.log("patched JS_ADDITIONAL_MODULES include_str! to .to_string()")
}
