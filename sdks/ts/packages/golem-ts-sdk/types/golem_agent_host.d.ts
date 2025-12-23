declare module 'golem:agent/host' {
  import * as golemAgentCommon from 'golem:agent/common';
  import * as golemRpc022Types from 'golem:rpc/types@0.2.2';
  /**
   * Gets all the registered agent types
   */
  export function getAllAgentTypes(): RegisteredAgentType[];
  /**
   * Get a specific registered agent type by name
   */
  export function getAgentType(agentTypeName: string): RegisteredAgentType | undefined;
  /**
   * Constructs a string agent-id from the agent type and its constructor parameters
   * and an optional phantom ID
   * @throws AgentError
   */
  export function makeAgentId(agentTypeName: string, input: DataValue, phantomId: Uuid | undefined): string;
  /**
   * Parses an agent-id (created by `make-agent-id`) into an agent type name and its constructor parameters
   * and an optional phantom ID
   * @throws AgentError
   */
  export function parseAgentId(agentId: string): [string, DataValue, Uuid | undefined];
  export type ComponentId = golemRpc022Types.ComponentId;
  export type Uuid = golemRpc022Types.Uuid;
  export type AgentError = golemAgentCommon.AgentError;
  export type AgentType = golemAgentCommon.AgentType;
  export type DataValue = golemAgentCommon.DataValue;
  /**
   * Associates an agent type with a component that implements it
   */
  export type RegisteredAgentType = {
    agentType: AgentType;
    implementedBy: ComponentId;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
