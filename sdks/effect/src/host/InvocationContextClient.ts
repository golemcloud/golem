/**
 * Host service around `golem:agent/invocation-context@2.0.0`. The agent, tool, and middleware
 * export entry points read the immediate caller from it.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Effect, Layer } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import * as InvocationContext from "golem:agent/invocation-context@2.0.0"

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

/** The principal that made the running invocation. */
export const immediateCaller: Effect.Effect<AgentCommon.Principal, never, InvocationContextClient> =
  Effect.gen(function* () {
    const client = yield* InvocationContextClient
    return yield* Effect.sync(() => client.caller().immediate)
  })
