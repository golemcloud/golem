/**
 * @internal
 * @since 1.5.0
 */
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as ApiHost from "golem:api/host@1.5.0"
import {
  dispatchDiscoverAgentTypes,
  dispatchGetDefinition,
  dispatchInitialize,
  dispatchInvoke,
  dispatchLoadSnapshot,
  dispatchSaveSnapshot,
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
 *
 * @since 1.5.0
 * @category runtime hooks
 */
export const guest: GuestExports = {
  initialize: dispatchInitialize,
  invoke: dispatchInvoke,
  discoverAgentTypes: dispatchDiscoverAgentTypes,
  getDefinition: dispatchGetDefinition,
}

/**
 * Snapshotting hooks. Wired through to the dispatchers in `./agent`.
 * Agents that don't declare a `snapshot` field map to
 * `snapshotting: { tag: "disabled" }`, so the host should never invoke
 * these for them; the dispatchers raise a clear error if it does.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
export const saveSnapshot: SaveSnapshotExports = {
  save: dispatchSaveSnapshot,
}

/**
 * Snapshot-restore hook. Wired through to {@link dispatchLoadSnapshot}
 * in `./agent`. The host calls `load` instead of `initialize` when an
 * agent is being restored from a snapshot envelope.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
export const loadSnapshot: LoadSnapshotExports = {
  load: dispatchLoadSnapshot,
}
