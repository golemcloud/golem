/** Schema-value RPC for callers that only have an agent ID. @since 1.6.0 */
import { Effect, Scope } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type { AgentId } from "./AgentId.js"
import {
  ClientBindingError,
  connectById,
  registeredTypeOf,
  type InvocationMetadata,
  type RemoteCallError,
} from "./Client.js"
import type { AgentHostClient } from "./host/AgentHostClient.js"
import type { RpcClient } from "./host/RpcClient.js"
import { dynamicMethod } from "./internal/dynamicMethod.js"
import type { GuestScopeCard } from "./internal/scopeCard.js"

/** @since 1.6.0 @category models */
export interface DynamicInvocation {
  readonly metadata: InvocationMetadata
  readonly value?: CoreTypes.SchemaValueTree
}

/**
 * Per-call options of an awaited dynamic or reflected call. `scopeCard` is borrowed for the
 * call; see `ScopeCardOptions.scopeCard` for its validity rules.
 *
 * @since 1.6.0
 * @category models
 */
export interface InvokeOptions {
  readonly scopeCard?: GuestScopeCard | undefined
}

/**
 * Schema-value RPC for one method. `invoke` never changes the caller-supplied tree: when the call
 * fails before the host receives it, the caller still owns the resources in it.
 *
 * @since 1.6.0
 * @category models
 */
export interface DynamicMethod {
  readonly name: string
  readonly invoke: (
    input: CoreTypes.SchemaValueTree,
    options?: InvokeOptions,
  ) => Effect.Effect<DynamicInvocation, RemoteCallError>
  readonly trigger: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<InvocationMetadata, RemoteCallError>
  readonly schedule: (
    at: AgentHost.Datetime,
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<
    { readonly metadata: InvocationMetadata; readonly cancel: Effect.Effect<void> },
    RemoteCallError,
    Scope.Scope
  >
}

/** @since 1.6.0 @category models */
export interface DynamicAgentClient {
  readonly agentId: AgentId
  readonly method: (name: string) => DynamicMethod
}

/**
 * Connect to the existing agent with the given ID, without a typed client definition. The agent
 * type comes from the type tag of the ID. An unknown ID fails with `not-found`; an ID of an
 * ephemeral agent type fails with `ClientBindingError`. Inputs and outputs are schema-value
 * trees, so the caller owns the independent method definitions.
 * @since 1.6.0
 * @category constructors
 */
export const bind = (
  id: AgentId,
): Effect.Effect<
  DynamicAgentClient,
  RemoteCallError | ClientBindingError,
  RpcClient | AgentHostClient | Scope.Scope
> =>
  Effect.gen(function* () {
    const { raw, registered } = yield* registeredTypeOf(id)
    if (registered.agentType.mode === "ephemeral")
      return yield* Effect.fail(
        new ClientBindingError(
          `Cannot bind agent ID '${id}' to ephemeral agent type '${registered.agentType.typeName}'`,
        ),
      )
    const rpc = yield* connectById(registered.agentType.typeName, raw)
    return Object.freeze({
      agentId: id,
      method: (name: string) => dynamicMethod(rpc, name),
    })
  })
