import { defineConfig } from "rolldown"

export default defineConfig({
  input: "src/main.ts",
  platform: "node",
  // Optional modules of `pg` that the billing app does not use.
  external: ["pg-native", "cloudflare:sockets"],
  output: {
    dir: "dist",
    format: "esm",
    entryFileNames: "main.mjs",
    codeSplitting: false,
    sourcemap: true
  }
})
