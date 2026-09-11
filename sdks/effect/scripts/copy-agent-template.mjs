#!/usr/bin/env node
/**
 * Copies the freshly-built agent-template WASM into wasm/agent_guest.wasm,
 * which is the artifact consumed by user applications via the Golem CLI's
 * `injectToPrebuiltQuickjs` build step.
 */
import { fileURLToPath } from "node:url"
import { dirname, resolve } from "node:path"
import { copyFileSync, existsSync, mkdirSync } from "node:fs"

const here = dirname(fileURLToPath(import.meta.url))
const root = resolve(here, "..")
const src = resolve(root, "agent-template/target/wasm32-wasip2/release/agent_guest.wasm")
const destDir = resolve(root, "wasm")
const dest = resolve(destDir, "agent_guest.wasm")

if (!existsSync(src)) {
  console.error(`error: source WASM not found: ${src}`)
  process.exit(1)
}

mkdirSync(destDir, { recursive: true })
copyFileSync(src, dest)
console.log(`copied ${src} -> ${dest}`)
