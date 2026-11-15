/**
 * Host service around `golem:agent/invocation-context@2.0.0`. The agent, tool, and middleware
 * export entry points build the per-invocation context from it.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Effect, Layer } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import * as InvocationContext from "golem:agent/invocation-context@2.0.0"
import type { InvocationContextShape } from "../InvocationContext.js"
import { lazyInvocationContext } from "../internal/invocationContext.js"

export interface InvocationContextClientShape {
  /** Mirrors `golem:agent/invocation-context.caller`. */
  readonly caller: () => AgentCommon.Caller
}

export class InvocationContextClient extends Context.Service<
  InvocationContextClient,
  InvocationContextClientShape
>()("effect-golem/host/InvocationContext") {}

export const InvocationContextLive: Layer.Layer<InvocationContextClient> = Layer.succeed(
  InvocationContextClient,
  InvocationContextClient.of({
    caller: () => InvocationContext.caller(),
  }),
)

/**
 * The context of the running invocation. The host is asked for the caller on first use only.
 */
export const currentInvocationContext: Effect.Effect<
  InvocationContextShape,
  never,
  InvocationContextClient
> = Effect.gen(function* () {
  const client = yield* InvocationContextClient
  return lazyInvocationContext(client.caller)
})
