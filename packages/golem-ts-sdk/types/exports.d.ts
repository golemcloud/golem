declare module 'agent-guest' {
  import * as golemAgentCommon from 'golem:agent/common';
  export namespace guest {
    /**
     * Initializes the agent of a given type with the given constructor parameters.
     * If called a second time, it fails.
     */
    export function initialize(agentType: string, input: DataValue): Promise<Result<void, AgentError>>;
    /**
     * Invokes an agent. If create was not called before, it fails
     */
    export function invoke(methodName: string, input: DataValue): Promise<Result<DataValue, AgentError>>;
    /**
     * Gets the agent type. If create was not called before, it fails
     */
    export function getDefinition(): Promise<AgentType>;
    /**
     * Gets the agent types defined by this component
     */
    export function discoverAgentTypes(): Promise<AgentType[]>;
    export type AgentError = golemAgentCommon.AgentError;
    export type AgentType = golemAgentCommon.AgentType;
    export type DataValue = golemAgentCommon.DataValue;
    export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
  }
}
