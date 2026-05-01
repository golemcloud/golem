/**
 * Public `Agent` namespace barrel.
 *
 * The implementation lives in `src/internal/agent.ts` (the dispatcher,
 * the registry, and the `userRuntimeLayer` host-services seam — see
 * the "Host injection seam" section in `AGENTS.md`); this facade
 * re-exports the public symbols that consumers — the package barrel,
 * the generated `agent-guest` shim in `internal/guest.ts`, the typed
 * RPC client builder in {@link Client}, and user agent code — reach
 * for.
 *
 * Mirrors the {@link Durability} facade pattern (see also the
 * "Module organisation conventions" section in `AGENTS.md`).
 *
 * @since 0.1.0
 * @category modules
 */
export * from "./internal/agent.js"
