declare module 'golem:agent/host@1.5.0' {
  import * as golemAgent150Common from 'golem:agent/common@1.5.0';
  import * as golemCore150Types from 'golem:core/types@1.5.0';
  import * as wasiClocks023WallClock from 'wasi:clocks/wall-clock@0.2.3';
  import * as wasiIo023Poll from 'wasi:io/poll@0.2.3';
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
   * * Only the agent type that created the promise is allowed to create a webhook for it. Using this host function
   *   from a different agent type will trap.
   */
  export function createWebhook(promiseId: PromiseId): string;
  export class WasmRpc {
    /**
     * Constructs the RPC client connecting to the given target agent
     */
    constructor(agentTypeName: string, constructor: DataValue, phantomId: Uuid | undefined);
    /**
     * Invokes a remote method with the given parameters, and awaits the result
     * @throws RpcError
     */
    invokeAndAwait(methodName: string, input: DataValue): DataValue;
    /**
     * Triggers the invocation of a remote method with the given parameters, and returns immediately.
     * @throws RpcError
     */
    invoke(methodName: string, input: DataValue): void;
    /**
     * Invokes a remote method with the given parameters, and returns a `future-invoke-result` value which can
     * be polled for the result.
     * With this function it is possible to call multiple (different) agents simultaneously.
     */
    asyncInvokeAndAwait(methodName: string, input: DataValue): FutureInvokeResult;
    /**
     * Schedule invocation for later
     */
    scheduleInvocation(scheduledTime: Datetime, methodName: string, input: DataValue): void;
    /**
     * Schedule invocation for later. Call cancel on the returned resource to cancel the invocation before the scheduled time.
     */
    scheduleCancelableInvocation(scheduledTime: Datetime, methodName: string, input: DataValue): CancellationToken;
  }
  export class FutureInvokeResult {
    /**
     * Subscribes to the result of the invocation
     */
    subscribe(): Pollable;
    /**
     * Poll for the invocation. If the invocation has not completed yet, returns `none`.
     */
    get(): Result<DataValue, RpcError> | undefined;
  }
  export class CancellationToken {
    /**
     * Cancel the scheduled invocation
     */
    cancel(): void;
  }
  export type ComponentId = golemCore150Types.ComponentId;
  export type Uuid = golemCore150Types.Uuid;
  export type PromiseId = golemCore150Types.PromiseId;
  export type Datetime = wasiClocks023WallClock.Datetime;
  export type Pollable = wasiIo023Poll.Pollable;
  export type AgentError = golemAgent150Common.AgentError;
  export type AgentType = golemAgent150Common.AgentType;
  export type DataValue = golemAgent150Common.DataValue;
  export type RegisteredAgentType = golemAgent150Common.RegisteredAgentType;
  /**
   * Possible failures of an RPC call
   */
  export type RpcError = 
  /** Protocol level error */
  {
    tag: 'protocol-error'
    val: string
  } |
  /** Access denied */
  {
    tag: 'denied'
    val: string
  } |
  /** Target agent or function not found */
  {
    tag: 'not-found'
    val: string
  } |
  /** Internal error on the remote side */
  {
    tag: 'remote-internal-error'
    val: string
  } |
  /** The remote endpoint returned an agent-domain error */
  {
    tag: 'remote-agent-error'
    val: AgentError
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
