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
  /**
   * Creates a webhook that can be used to integrate with webhook driven apis.
   * When the created url is called with a post request, the provided promise-id is completed with the body of the post request.
   * Note the following behaviours:
   * * Only agents whoose agent types are _currently_ deployed via an http api are allowed to create a webhook. Calling this function while the agent
   *    is not deployed via an http api will trap.
   */
  export function createWebhook(promiseId: PromiseId): string;
  export type ComponentId = golemRpc022Types.ComponentId;
  export type Uuid = golemRpc022Types.Uuid;
  export type PromiseId = golemRpc022Types.PromiseId;
  export type AgentError = golemAgentCommon.AgentError;
  export type AgentType = golemAgentCommon.AgentType;
  export type DataValue = golemAgentCommon.DataValue;
  export type RegisteredAgentType = golemAgentCommon.RegisteredAgentType;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
