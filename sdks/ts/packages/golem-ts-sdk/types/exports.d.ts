declare module 'agent-guest' {
  import * as golemAgentCommon from 'golem:agent/common';
  import * as golemApi130Host from 'golem:api/host@1.3.0';
  /**
   * Interface providing user-defined snapshotting capability. This can be used to perform manual update of agents
   * when the new component incompatible with the old one.
   */
  export namespace saveSnapshot {
    /**
     * Saves the component's state into a user-defined snapshot
     */
    export function save(): Promise<Snapshot>;
    export type Snapshot = golemApi130Host.Snapshot;
  }
  /**
   * Interface providing user-defined snapshotting capability. This can be used to perform manual update of agents
   * when the new component incompatible with the old one.
   */
  export namespace loadSnapshot {
    /**
     * Tries to load a user-defined snapshot, setting up the agent's state based on it.
     * The function can return with a failure to indicate that the update is not possible.
     * @throws string
     */
    export function load(snapshot: Snapshot): Promise<void>;
    export type Snapshot = golemApi130Host.Snapshot;
    export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
  }
  export namespace guest {
    /**
     * Initializes the agent of a given type with the given constructor parameters.
     * If called a second time, it fails.
     * @throws AgentError
     */
    export function initialize(agentType: string, input: DataValue, principal: Principal): Promise<void>;
    /**
     * Invokes an agent. If create was not called before, it fails
     * @throws AgentError
     */
    export function invoke(methodName: string, input: DataValue, principal: Principal): Promise<DataValue>;
    /**
     * Gets the agent type. If create was not called before, it fails
     */
    export function getDefinition(): Promise<AgentType>;
    /**
     * Gets the agent types defined by this component
     * @throws AgentError
     */
    export function discoverAgentTypes(): Promise<AgentType[]>;
    export type AgentError = golemAgentCommon.AgentError;
    export type AgentType = golemAgentCommon.AgentType;
    export type DataValue = golemAgentCommon.DataValue;
    export type Principal = golemAgentCommon.Principal;
    export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
  }
}
