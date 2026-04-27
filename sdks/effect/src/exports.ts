import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as ApiHost from "golem:api/host@1.5.0"
import {
  dispatchDiscoverAgentTypes,
  dispatchGetDefinition,
  dispatchInitialize,
  dispatchInvoke,
} from "./agent.js"

/**
 * Shape of the `golem:agent/guest@1.5.0` interface that the base WASM
 * exposes via the `agent-guest` module. Inlined here (instead of
 * imported via `import type * as bindings from "agent-guest"`) so the
 * compiled `.d.ts` stays self-contained and consumers of the published
 * package don't have to register the ambient `agent-guest` declaration.
 */
interface GuestExports {
  initialize: (
    agentType: string,
    input: AgentCommon.DataValue,
    principal: AgentCommon.Principal,
  ) => Promise<void>
  invoke: (
    methodName: string,
    input: AgentCommon.DataValue,
    principal: AgentCommon.Principal,
  ) => Promise<AgentCommon.DataValue>
  getDefinition: () => Promise<AgentCommon.AgentType>
  discoverAgentTypes: () => Promise<AgentCommon.AgentType[]>
}

interface SaveSnapshotExports {
  save: () => Promise<ApiHost.Snapshot>
}

interface LoadSnapshotExports {
  load: (snapshot: ApiHost.Snapshot) => Promise<void>
}

/**
 * Implementations of the Golem `agent-guest` host exports. Re-exported
 * from {@link ./index} as the package's public surface so that the
 * generated WIT bindings can find them at the package entry point.
 *
 * Users of this library define their agents/methods declaratively via
 * `defineAgent` + `defineMethod`, and `registerAgent` wires them into
 * these dispatchers — they should never need to interact with `guest`,
 * `saveSnapshot`, or `loadSnapshot` directly.
 */
export const guest: GuestExports = {
  initialize: dispatchInitialize,
  invoke: dispatchInvoke,
  discoverAgentTypes: dispatchDiscoverAgentTypes,
  getDefinition: dispatchGetDefinition,
}

/**
 * Snapshotting is not yet implemented. Both functions throw at runtime
 * if Golem actually invokes them; agents that opt out via
 * `snapshotting: { tag: "disabled" }` (the default) will never trigger
 * these calls.
 */
export const saveSnapshot: SaveSnapshotExports = {
  save: async () => {
    throw new Error("saveSnapshot.save is not implemented")
  },
}

export const loadSnapshot: LoadSnapshotExports = {
  load: async () => {
    throw new Error("loadSnapshot.load is not implemented")
  },
}
