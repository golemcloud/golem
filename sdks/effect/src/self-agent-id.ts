/**
 * @since 0.1.0
 */
import { Context } from "effect"
import type * as CoreTypes from "golem:core/types@1.5.0"

/**
 * Effect service exposing the running agent's own structured
 * {@link CoreTypes.AgentId}.
 *
 * Captured once by the dispatcher (via a single `getSelfMetadata` call
 * at agent `initialize` / `load-snapshot`) and provided to all
 * subsequent constructor (`impl`) and per-method effects. Resolving
 * this is free — no host call, no oplog entry — which makes it the
 * right primitive for self-targeting wrappers like
 * {@link Durability.unwrapOrRevert} and `Agents.fork` defaults.
 *
 * For richer fields (component revision, status, retry count, env,
 * config), use `Agents.getSelfMetadata` directly.
 *
 * Outside the dispatcher (e.g. unit tests) this service must be
 * provided explicitly via `Effect.provideService(SelfAgentId, …)`,
 * mirroring how {@link Principal} works.
 *
 * @since 0.1.0
 * @category host services
 */
export class SelfAgentId extends Context.Service<SelfAgentId, CoreTypes.AgentId>()(
  "effect-golem/SelfAgentId",
) {}
