/** Injectable host service for the runtime's custom invocation context. */
import { Context, Layer } from "effect"
import type * as DurabilityHost from "golem:durability/durability@1.6.0"
import * as Runtime from "wasm-rquickjs:custom-invocation-context"

export type CustomScopeToken = Runtime.Token

export interface CustomInvocationContextClientShape {
  readonly capture: () => CustomScopeToken
  readonly swap: (next: CustomScopeToken) => CustomScopeToken
  readonly run: <A>(token: CustomScopeToken, fn: () => A) => A
  readonly enterLive: (live: DurabilityHost.LiveCustomDurableInvocation) => CustomScopeToken
  readonly close: (token: CustomScopeToken) => void
}

export class CustomInvocationContextClient extends Context.Service<
  CustomInvocationContextClient,
  CustomInvocationContextClientShape
>()("effect-golem/host/CustomInvocationContext") {}

export const CustomInvocationContextLive: Layer.Layer<CustomInvocationContextClient> =
  Layer.succeed(
    CustomInvocationContextClient,
    CustomInvocationContextClient.of({
      capture: () => Runtime.capture(),
      swap: (next) => Runtime.swap(next),
      run: (token, fn) => Runtime.run(token, fn),
      enterLive: (live) => Runtime.enterLive(live),
      close: (token) => Runtime.close(token),
    }),
  )
