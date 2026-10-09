/**
 * @since 1.5.0
 */
import { Effect } from "effect"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { AgentHostClient } from "./host/AgentHostClient.js"

/**
 * Read the running agent's own structured
 * {@link CoreTypes.AgentId}.
 *
 * Each execution reads durable host metadata without caching. During
 * replay it reproduces the identity observed at that point in history;
 * new live reads after a fork identify the child, even within the same
 * invocation. Use `yield* SelfAgentId.SelfAgentId` in initialization,
 * methods, or snapshot effects. Values explicitly saved in application
 * state are ordinary state and are not rebound after a fork.
 *
 * For richer fields (component revision, status, retry count, env,
 * config), use `Agents.getSelfMetadata` directly.
 *
 * Outside the dispatcher (e.g. unit tests), provide `AgentHostClient`
 * with a host implementation. Host failures become Effect defects.
 *
 * @since 1.5.0
 * @category host services
 */
export const SelfAgentId: Effect.Effect<CoreTypes.AgentId, never, AgentHostClient> = Effect.gen(
  function* () {
    const host = yield* AgentHostClient
    return yield* Effect.sync(() => host.getSelfMetadata().agentId)
  },
)
