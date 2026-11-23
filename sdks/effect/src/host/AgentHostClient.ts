/**
 * Host service bundling the agent-metadata + webhook subset of two
 * related WIT interfaces:
 *
 * - `golem:agent/host@2.0.0` agent-type discovery (`getAgentType`,
 *   `getAgentTypeByAgentId`, `getAllAgentTypes`), used by `Reflection`
 *   and by binding a client to an agent ID.
 * - `golem:agent/host@2.0.0.createWebhook` — mints a public POST URL
 *   bound to a previously-allocated host promise. Used by
 *   `src/webhook.ts:create`.
 * - `golem:api/host@1.5.0.getSelfMetadata` — reads the running agent's
 *   full metadata. Used by the uncached `SelfAgentId` effect and by
 *   `src/agents.ts:getSelfMetadata` for user-facing reads.
 * - `golem:api/host@1.5.0.getAgentMetadata` / `updateAgent` /
 *   `forkAgent` / `revertAgent` / `fork` / `resolveComponentId` /
 *   `GetAgents` — agent-lifecycle and enumeration calls used by
 *   `src/agents.ts`.
 *
 * They are grouped here because they all answer the same
 * "who am I?" / "where am I reachable?" / "who else is around?"
 * question, even though several live in `golem:api/host` rather than
 * `golem:agent/host`.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import * as AgentHost from "golem:agent/host@2.0.0"
import * as ApiHost from "golem:api/host@1.5.0"
import type * as CoreTypes from "golem:core/types@2.0.0"

export interface AgentHostClientShape {
  /** Mirrors optional agent-type discovery by name. */
  readonly getAgentType: (name: string) => AgentHost.RegisteredAgentType | undefined
  /** Mirrors discovery of the deployed agent type that the type tag of an agent ID names. */
  readonly getAgentTypeByAgentId: (
    agentId: CoreTypes.AgentId,
  ) => AgentHost.RegisteredAgentType | undefined
  /** Mirrors discovery of every visible agent type. */
  readonly getAllAgentTypes: () => ReadonlyArray<AgentHost.RegisteredAgentType>
  /**
   * Mirrors `golem:api/host.getSelfMetadata`. Synchronous; errors
   * thrown by the host become Effect defects when wrapped at the call
   * site.
   */
  readonly getSelfMetadata: () => ApiHost.AgentMetadata
  /**
   * Mirrors `golem:agent/host.createWebhook`. Synchronous; errors
   * thrown by the host (e.g. agent-not-deployed-via-http-api,
   * cross-component promise reuse) become Effect defects when wrapped
   * at the call site (see `src/webhook.ts:create`, which routes them
   * into a typed `WebhookHostError`).
   */
  readonly createWebhook: (promiseId: ApiHost.PromiseId) => string
  /** Mirrors `golem:api/host.getAgentMetadata`. */
  readonly getAgentMetadata: (id: ApiHost.AgentId) => ApiHost.AgentMetadata | undefined
  /** Mirrors `golem:api/host.updateAgent`. */
  readonly updateAgent: (
    id: ApiHost.AgentId,
    targetRevision: ApiHost.ComponentRevision,
    mode: ApiHost.UpdateMode,
  ) => void
  /** Mirrors `golem:api/host.forkAgent`; returns the ID that Golem issued for the new agent. */
  readonly forkAgent: (
    source: ApiHost.AgentId,
    oplogIdxCutOff: ApiHost.OplogIndex,
  ) => ApiHost.AgentId
  /** Mirrors `golem:api/host.revertAgent`. */
  readonly revertAgent: (id: ApiHost.AgentId, target: ApiHost.RevertAgentTarget) => void
  /** Mirrors `golem:api/host.fork`. */
  readonly fork: () => ApiHost.ForkResult
  /** Mirrors `golem:api/host.resolveComponentId`. */
  readonly resolveComponentId: (componentReference: string) => ApiHost.ComponentId | undefined
  /** Mirrors `new GetAgents(agentTypeName, filter, precise)`. */
  readonly getAgentsCtor: (
    agentTypeName: string,
    filter: ApiHost.AgentAnyFilter | undefined,
    precise: boolean,
  ) => ApiHost.GetAgents
  /**
   * Mirrors `golem:api/host.trap`. Unconditionally surfaces as an
   * uncatchable wasm trap on the host side; from JavaScript's
   * perspective the call never returns and the surrounding fiber's
   * frame is torn down by the runtime. Saga entry points use this for
   * unexpected defects, not for recoverable typed failures.
   */
  readonly trap: (reason: string) => never
}

export class AgentHostClient extends Context.Service<AgentHostClient, AgentHostClientShape>()(
  "effect-golem/host/AgentHost",
) {}

export const AgentHostLive: Layer.Layer<AgentHostClient> = Layer.succeed(
  AgentHostClient,
  AgentHostClient.of({
    getAgentType: (name) => AgentHost.getAgentType(name),
    getAgentTypeByAgentId: (agentId) => AgentHost.getAgentTypeByAgentId(agentId),
    getAllAgentTypes: () => AgentHost.getAllAgentTypes(),
    getSelfMetadata: () => ApiHost.getSelfMetadata(),
    createWebhook: (promiseId) => AgentHost.createWebhook(promiseId),
    getAgentMetadata: (id) => ApiHost.getAgentMetadata(id),
    updateAgent: (id, t, m) => ApiHost.updateAgent(id, t, m),
    forkAgent: (s, o) => ApiHost.forkAgent(s, o),
    revertAgent: (id, t) => ApiHost.revertAgent(id, t),
    fork: () => ApiHost.fork(),
    resolveComponentId: (r) => ApiHost.resolveComponentId(r),
    getAgentsCtor: (t, f, p) => new ApiHost.GetAgents(t, f, p),
    trap: (reason) => {
      ApiHost.trap(reason)
      // The line above never returns at runtime (the host call is an
      // uncatchable wasm trap). The throw keeps `never` happy on the
      // type level for environments where the WIT binding is replaced
      // by a no-op stub (tests).
      throw new Error(`golem:api/host.trap returned (reason: ${reason})`)
    },
  }),
)
