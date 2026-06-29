declare module 'golem:agent/host@2.0.0' {
  import * as golemAgent200Common from 'golem:agent/common@2.0.0';
  import * as golemCore200Types from 'golem:core/types@2.0.0';
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
   * and an optional phantom ID.
   * `input` is a value tree whose root encodes the constructor's parameter list.
   * @throws AgentError
   */
  export function makeAgentId(agentTypeName: string, input: SchemaValueTree, phantomId: Uuid | undefined): string;
  /**
   * Parses an agent-id (created by `make-agent-id`) into an agent type name and its constructor parameters
   * and an optional phantom ID.
   * The constructor parameters are returned as a self-contained typed value
   * (graph + value tree) so the receiver can interpret them without an
   * external schema registry.
   * @throws AgentError
   */
  export function parseAgentId(agentId: string): [string, TypedSchemaValue, Uuid | undefined];
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
  /**
   * Get the current value of the config key.
   * The expected schema is a hint to the host what type of value is expected by the guest and can be used
   * by the host to automatically migrate config values to fit the expected schema.
   * Only keys that are declared by the agent-type are allowed to be accessed. Trying
   * to access an undeclared key will trap, unless the expected type is an option. In that case
   * none is returned.
   * Getting a local key will get values defined as part of the current
   * component revision + overrides declared during agent creation.
   * Getting a shared key will get the current value of the key in the environment.
   */
  export function getConfigValue(key: string[], expected: SchemaGraph): SchemaValueTree;
  export class WasmRpc {
    /**
     * Constructs the RPC client connecting to the given target agent.
     * `constructor` is a value tree whose root encodes the target agent
     * constructor's parameter list.
     */
    constructor(agentTypeName: string, constructor: SchemaValueTree, phantomId: Uuid | undefined, agentConfig: TypedAgentConfigValue[]);
    /**
     * Invokes a remote method with the given parameters, and awaits the result.
     * `input` encodes the method's parameter list; the result is `none` for
     * a `unit` output and `some(value)` for a `single` output.
     * @throws RpcError
     */
    invokeAndAwait(methodName: string, input: SchemaValueTree): SchemaValueTree | undefined;
    /**
     * Triggers the invocation of a remote method with the given parameters, and returns immediately.
     * @throws RpcError
     */
    invoke(methodName: string, input: SchemaValueTree): void;
    /**
     * Invokes a remote method with the given parameters, and returns a `future-invoke-result` value which can
     * be polled for the result.
     * With this function it is possible to call multiple (different) agents simultaneously.
     */
    asyncInvokeAndAwait(methodName: string, input: SchemaValueTree): FutureInvokeResult;
    /**
     * Schedule invocation for later
     */
    scheduleInvocation(scheduledTime: Datetime, methodName: string, input: SchemaValueTree): void;
    /**
     * Schedule invocation for later. Call cancel on the returned resource to cancel the invocation before the scheduled time.
     */
    scheduleCancelableInvocation(scheduledTime: Datetime, methodName: string, input: SchemaValueTree): CancellationToken;
  }
  export class FutureInvokeResult {
    /**
     * Subscribes to the result of the invocation
     */
    subscribe(): Pollable;
    /**
     * Poll for the invocation. If the invocation has not completed yet, returns `none`.
     */
    get(): Result<SchemaValueTree | undefined, RpcError> | undefined;
    /**
     * Best-effort attempt to cancel the remote invocation by idempotency key.
     * If the invocation has already started or completed, this is a no-op.
     */
    cancel(): void;
  }
  export class CancellationToken {
    /**
     * Cancel the scheduled invocation
     */
    cancel(): void;
  }
  export type ComponentId = golemCore200Types.ComponentId;
  export type Uuid = golemCore200Types.Uuid;
  export type PromiseId = golemCore200Types.PromiseId;
  export type SchemaGraph = golemCore200Types.SchemaGraph;
  export type SchemaValueTree = golemCore200Types.SchemaValueTree;
  export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
  export type Datetime = wasiClocks023WallClock.Datetime;
  export type Pollable = wasiIo023Poll.Pollable;
  export type AgentError = golemAgent200Common.AgentError;
  export type AgentType = golemAgent200Common.AgentType;
  export type RegisteredAgentType = golemAgent200Common.RegisteredAgentType;
  export type TypedAgentConfigValue = golemAgent200Common.TypedAgentConfigValue;
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
