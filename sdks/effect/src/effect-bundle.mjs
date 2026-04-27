/**
 * Bundle entry that re-exports the entire `effect` package's main
 * module. Rollup compiles this into `dist/effect.mjs`, which is then
 * embedded as a standalone JS module inside the base WASM (next to
 * `effect-golem` and the `user` slot) so that all components share one
 * Effect runtime instance.
 */
export * from "effect"
