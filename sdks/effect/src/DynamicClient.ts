/** Schema-value RPC for callers that only have an environment-scoped agent ID. @since 1.6.0 */
import { Effect, Scope } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { AgentIdentityError, rawPhantomId, type Identity } from "./AgentIdentity.js"
import type { RemoteCallError } from "./Client.js"
import { RpcClient } from "./host/RpcClient.js"
import { dynamicMethod } from "./internal/dynamicMethod.js"
import { wrapHostThrow } from "./internal/rpc.js"

/** @since 1.6.0 @category models */
export interface DynamicInvocation {
  readonly metadata: AgentHost.InvocationMetadata
  readonly value?: CoreTypes.SchemaValueTree
}

/** @since 1.6.0 @category models */
export interface DynamicMethod {
  readonly name: string
  readonly invoke: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<DynamicInvocation, RemoteCallError>
  readonly trigger: (
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<AgentHost.InvocationMetadata, RemoteCallError>
  readonly schedule: (
    at: AgentHost.Datetime,
    input: CoreTypes.SchemaValueTree,
  ) => Effect.Effect<
    { readonly metadata: AgentHost.InvocationMetadata; readonly cancel: Effect.Effect<void> },
    RemoteCallError,
    Scope.Scope
  >
}

/** @since 1.6.0 @category models */
export interface DynamicAgentClient {
  readonly agentId: string
  readonly method: (name: string) => DynamicMethod
}

/**
 * Bind directly to an agent ID without a reflection lookup. Inputs and outputs
 * are schema-value trees, so the caller owns the independent method definitions.
 * @since 1.6.0
 * @category constructors
 */
export const bind = (
  identity: Identity,
): Effect.Effect<
  DynamicAgentClient,
  RemoteCallError | AgentIdentityError,
  RpcClient | Scope.Scope
> =>
  Effect.gen(function* () {
    const phantom = yield* Effect.try({
      try: () => rawPhantomId(identity),
      catch: (cause) => new AgentIdentityError(cause),
    })
    const rpcHost = yield* RpcClient
    const rpc = yield* Effect.acquireRelease(
      Effect.mapError(
        rpcHost.connect(identity.typeName, identity.constructorValue, phantom, []),
        (error) => wrapHostThrow(error.cause),
      ),
      (connection) => Effect.sync(() => connection.drop()),
    )
    return Object.freeze({
      agentId: identity.encoded,
      method: (name: string) => dynamicMethod(rpc, name),
    })
  })
