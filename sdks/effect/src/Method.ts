/**
 * Public `Method` namespace barrel.
 *
 * The implementation lives in `src/internal/method.ts`; this facade
 * re-exports the public symbols that consumers — the package barrel,
 * `defineAgent` in `internal/agent.ts`, the typed RPC client builder
 * in {@link Client}, and user agent code — reach for.
 *
 * Mirrors the {@link Durability} facade pattern (see also the
 * "Module organisation conventions" section in `AGENTS.md`).
 *
 * @since 0.1.0
 * @category modules
 */
export * from "./internal/method.js"
