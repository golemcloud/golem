/** Identity host edges and remote binding, without public identity wrapping or ownership. */
import { Effect } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { parseUuid } from "golem:core/types@2.0.0"
import type { Identity } from "../AgentIdentity.js"
import { AgentHostClient, type AgentHostClientShape } from "../host/AgentHostClient.js"
import { RpcClient } from "../host/RpcClient.js"
import { wrapHostThrow } from "./rpc.js"
import { rawPhantomId } from "./agentIdentityState.js"
import { withCapabilityAdoptionTransaction } from "./schema-model/capabilityTransaction.js"
import type { SchemaValue, TypedSchemaValue } from "./schema-model/model.js"
import {
  restoreEncodedCapabilitiesOnAbort,
  schemaValueToWitAsync,
  typedSchemaValueToWit,
} from "./schema-model/wit.js"

export const decodeAgentId = (host: Pick<AgentHostClientShape, "parseAgentId">, encoded: string) =>
  host.parseAgentId(encoded)

export const encodeAgentId = (
  host: Pick<AgentHostClientShape, "makeAgentId">,
  typeName: string,
  constructorValue: CoreTypes.SchemaValueTree,
  phantom: CoreTypes.Uuid | undefined,
) => host.makeAgentId(typeName, constructorValue, phantom)

export const connectIdentity = <E>(
  identity: Identity,
  entries: ReadonlyArray<AgentCommon.TypedAgentConfigValue>,
  identityError: (cause: unknown) => E,
) =>
  Effect.gen(function* () {
    const phantom = yield* Effect.try({
      try: () => rawPhantomId(identity),
      catch: identityError,
    })
    const host = yield* RpcClient
    return yield* host
      .connect(identity.typeName, identity.constructorValue, phantom, entries)
      .pipe(Effect.mapError((error) => wrapHostThrow(error.cause)))
  })

/** The caller retains scoped ownership of the returned connection effect. */
export const prepareRemoteAgent = (
  typeName: string,
  constructor: SchemaValue,
  phantomId: string | undefined,
  configs: readonly { readonly path: readonly string[]; readonly value: TypedSchemaValue }[],
  mode: "durable" | "ephemeral",
) =>
  Effect.gen(function* () {
    const constructorTree = yield* Effect.tryPromise({
      try: (signal) =>
        restoreEncodedCapabilitiesOnAbort(signal, schemaValueToWitAsync(constructor, signal)),
      catch: wrapHostThrow,
    })
    const phantom =
      phantomId === undefined
        ? undefined
        : yield* Effect.try({ try: () => parseUuid(phantomId), catch: wrapHostThrow })
    const agentHost = yield* AgentHostClient
    const agentId =
      mode === "durable"
        ? yield* Effect.try({
            try: () => encodeAgentId(agentHost, typeName, constructorTree, phantom),
            catch: wrapHostThrow,
          })
        : typeName
    const rpcHost = yield* RpcClient
    const wireConfigs = yield* Effect.try({
      try: () =>
        withCapabilityAdoptionTransaction(() =>
          configs.map(({ path, value }) => ({
            path: [...path],
            value: typedSchemaValueToWit(value),
          })),
        ),
      catch: wrapHostThrow,
    })
    return {
      agentId,
      connect: rpcHost.connect(typeName, constructorTree, phantom, wireConfigs),
    }
  })
