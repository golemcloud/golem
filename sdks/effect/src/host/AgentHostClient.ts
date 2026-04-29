/**
 * Host service bundling the agent-metadata + webhook subset of two
 * related WIT interfaces:
 *
 * - `golem:agent/host@1.5.0.parseAgentId` — recovers a structured
 *   `(typeName, ctorDataValue, phantomId?)` tuple from the
 *   `GOLEM_AGENT_ID` env var. Used by the snapshot-load dispatcher.
 * - `golem:agent/host@1.5.0.createWebhook` — mints a public POST URL
 *   bound to a previously-allocated host promise. Used by
 *   `src/webhook.ts:create`.
 * - `golem:api/host@1.5.0.getSelfMetadata` — reads the running agent's
 *   full metadata. Used by the agent dispatcher to capture the
 *   structured `SelfAgentId` once at agent-init time, and by
 *   `src/agents.ts:getSelfMetadata` for user-facing reads.
 * - `golem:api/host@1.5.0.getAgentMetadata` / `updateAgent` /
 *   `forkAgent` / `revertAgent` / `fork` — agent-lifecycle calls used
 *   by `src/agents.ts`.
 * - `golem:api/host@1.5.0.resolveComponentId` / `resolveAgentId` /
 *   `resolveAgentIdStrict` — id-resolution helpers used by
 *   `src/agents.ts`.
 * - `golem:api/host@1.5.0.GetAgents` — paged-iterator constructor used
 *   by `src/agents.ts:getAgents`.
 *
 * Pragmatically grouped here because they all answer the same
 * "who am I?" / "where am I reachable?" / "who else is around?"
 * question, even though several live in `golem:api/host` rather than
 * `golem:agent/host`. The eventual plan splits these further into
 * `ParseAgentIdClient` + `WebhookClient` + `AgentMetadataClient`; for
 * now the bundle keeps the wiring footprint small.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Layer } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import * as AgentHost from "golem:agent/host@1.5.0"
import * as ApiHost from "golem:api/host@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"

export interface AgentHostClientShape {
  /**
   * Mirrors `golem:agent/host.parseAgentId`. Errors thrown by the host
   * become Effect defects when wrapped at the call site.
   */
  readonly parseAgentId: (
    agentId: string,
  ) => [string, AgentCommon.DataValue, CoreTypes.Uuid | undefined]
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
  /** Mirrors `golem:api/host.forkAgent`. */
  readonly forkAgent: (
    source: ApiHost.AgentId,
    target: ApiHost.AgentId,
    oplogIdxCutOff: ApiHost.OplogIndex,
  ) => void
  /** Mirrors `golem:api/host.revertAgent`. */
  readonly revertAgent: (id: ApiHost.AgentId, target: ApiHost.RevertAgentTarget) => void
  /** Mirrors `golem:api/host.fork`. */
  readonly fork: () => ApiHost.ForkResult
  /** Mirrors `golem:api/host.resolveComponentId`. */
  readonly resolveComponentId: (componentReference: string) => ApiHost.ComponentId | undefined
  /** Mirrors `golem:api/host.resolveAgentId`. */
  readonly resolveAgentId: (
    componentReference: string,
    agentName: string,
  ) => ApiHost.AgentId | undefined
  /** Mirrors `golem:api/host.resolveAgentIdStrict`. */
  readonly resolveAgentIdStrict: (
    componentReference: string,
    agentName: string,
  ) => ApiHost.AgentId | undefined
  /** Mirrors `new GetAgents(componentId, filter, precise)`. */
  readonly getAgentsCtor: (
    componentId: ApiHost.ComponentId,
    filter: ApiHost.AgentAnyFilter | undefined,
    precise: boolean,
  ) => ApiHost.GetAgents
}

export class AgentHostClient extends Context.Service<AgentHostClient, AgentHostClientShape>()(
  "effect-golem/host/AgentHost",
) {}

export const AgentHostLive: Layer.Layer<AgentHostClient> = Layer.succeed(
  AgentHostClient,
  AgentHostClient.of({
    parseAgentId: (agentId) => AgentHost.parseAgentId(agentId),
    getSelfMetadata: () => ApiHost.getSelfMetadata(),
    createWebhook: (promiseId) => AgentHost.createWebhook(promiseId),
    getAgentMetadata: (id) => ApiHost.getAgentMetadata(id),
    updateAgent: (id, t, m) => ApiHost.updateAgent(id, t, m),
    forkAgent: (s, t, o) => ApiHost.forkAgent(s, t, o),
    revertAgent: (id, t) => ApiHost.revertAgent(id, t),
    fork: () => ApiHost.fork(),
    resolveComponentId: (r) => ApiHost.resolveComponentId(r),
    resolveAgentId: (r, n) => ApiHost.resolveAgentId(r, n),
    resolveAgentIdStrict: (r, n) => ApiHost.resolveAgentIdStrict(r, n),
    getAgentsCtor: (c, f, p) => new ApiHost.GetAgents(c, f, p),
  }),
)
