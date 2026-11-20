declare module 'golem:agent/host@2.0.0' {
  import * as golemAgent200Common from 'golem:agent/common@2.0.0';
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as wasiClocks030SystemClock from 'wasi:clocks/system-clock@0.3.0';
  /**
   * Gets all the registered agent types
   */
  export function getAllAgentTypes(): RegisteredAgentType[];
  /**
   * Get a specific registered agent type by name
   */
  export function getAgentType(agentTypeName: string): RegisteredAgentType | undefined;
  /**
   * Gets the deployed agent type that the type tag of an agent ID names. Returns `none`
   * when no deployed agent type has that tag.
   */
  export function getAgentTypeByAgentId(agentId: AgentId): RegisteredAgentType | undefined;
  /**
   * @throws WebhookError
   */
  export function createWebhook(promiseId: PromiseId): string;
  /**
   * Gets the ID of the keyed agent of the given type and constructor parameters, or
   * `none` when no such agent exists. Never creates the agent.
   * `input` is a value tree whose root encodes the constructor's parameter list.
   * @throws RpcError
   */
  export function lookupAgentId(agentTypeName: string, input: SchemaValueTree): AgentId | undefined;
  /**
   * @throws ConfigValueError
   */
  export function getConfigValue(key: string[], expected: SchemaGraph): SchemaValueTree;
  export class WasmRpc {
    /**
     * Creates an RPC client connecting to the keyed agent of the given type and
     * constructor parameters.
     * For durable targets, creates the target if missing and returns after successful
     * initialization. An initialized target is reused without rerunning its constructor.
     * Use `bind-self` to schedule on self from inside an unfinished constructor.
     * For ephemeral targets, each invocation runs on a new one-shot instance.
     * `constructor` is a value tree whose root encodes the target agent
     * constructor's parameter list.
     * @throws RpcError
     */
    static upsert(agentTypeName: string, constructor: SchemaValueTree): WasmRpc;
    /**
     * Creates an RPC client connecting to the existing agent with the given ID. Never
     * creates the agent. Fails with `not-found` when no agent of the given type has the ID.
     * @throws RpcError
     */
    static getById(agentTypeName: string, agentId: AgentId): WasmRpc;
    /**
     * Creates a durable agent of the given type, and returns after its successful
     * initialization. The creation is durable: replay returns the same agent.
     * `constructor` is a value tree whose root encodes the target agent
     * constructor's parameter list.
     * @throws RpcError
     */
    static create(agentTypeName: string, constructor: SchemaValueTree, options: CreateOptions): Promise<Created>;
    /**
     * The ID of the agent this client connects to. On a client of `upsert` it creates the
     * agent when it is missing, without invoking a method.
     * @throws RpcError
     */
    agentId(): Promise<AgentId>;
    /**
     * Binds the current durable agent incarnation without creating it or waiting
     * for its constructor to finish.
     * Use this to schedule work on self from inside the constructor. The agent's
     * identity already exists then, but initialization is still in progress.
     * Normal RPC client construction reuses an initialized agent without running
     * its constructor again, but waits if initialization is still in progress.
     * Calling it on self from the constructor would therefore deadlock.
     * Binding does not make awaited self-calls reentrant. Scheduled work runs
     * after construction finishes. Method calls still require method authority.
     * Typed clients must validate the returned agent type. The returned ID and
     * connection preserve the recorded incarnation during replay.
     * @throws RpcError
     */
    static bindSelf(): [AgentId, WasmRpc];
    /**
     * Invokes a remote method with the given parameters, and awaits the result.
     * `input` encodes the method's parameter list. The returned result is
     * `none` for a `unit` output and `some(value)` for a `single` output.
     * @throws RpcError
     */
    invokeAndAwait(methodName: string, input: SchemaValueTree, scopeCard: PermissionCard | undefined): InvocationResultWithMetadata;
    /**
     * Triggers the invocation of a remote method with the given parameters,
     * and returns its final identity immediately.
     * Scope lending is not supported: `scope-card` must be `none`.
     * @throws RpcError
     */
    invoke(methodName: string, input: SchemaValueTree, scopeCard: PermissionCard | undefined): InvocationMetadata;
    /**
     * Invokes a remote method with the given parameters, and returns a `future-invoke-result` value which can
     * be polled for the result together with the final invocation identity.
     * With this function it is possible to call multiple (different) agents simultaneously.
     */
    asyncInvokeAndAwait(methodName: string, input: SchemaValueTree, scopeCard: PermissionCard | undefined): AsyncInvocationWithMetadata;
    /**
     * Schedules an invocation for later and returns its final identity.
     * Scope lending is not supported: `scope-card` must be `none`.
     * @throws RpcError
     */
    scheduleInvocation(scheduledTime: Datetime, methodName: string, input: SchemaValueTree, scopeCard: PermissionCard | undefined): ScheduledInvocationReceipt;
    /**
     * Schedules an invocation for later and returns its final identity and
     * cancellation capability. Call cancel on the returned resource to
     * cancel the invocation before the scheduled time.
     * Scope lending is not supported: `scope-card` must be `none`.
     * @throws RpcError
     */
    scheduleCancelableInvocation(scheduledTime: Datetime, methodName: string, input: SchemaValueTree, scopeCard: PermissionCard | undefined): CancelableScheduledInvocationReceipt;
  }
  export class FutureInvokeResult {
    /**
     * Awaits the result of the invocation.
     * @throws RpcError
     */
    get(): Promise<SchemaValueTree | undefined>;
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
  export type AgentId = golemCore200Types.AgentId;
  export type PromiseId = golemCore200Types.PromiseId;
  export type SchemaGraph = golemCore200Types.SchemaGraph;
  export type SchemaValueTree = golemCore200Types.SchemaValueTree;
  export type PermissionCard = golemCore200Types.PermissionCard;
  export type InvocationId = golemCore200Types.InvocationId;
  export type Datetime = wasiClocks030SystemClock.Instant;
  export type AgentError = golemAgent200Common.AgentError;
  export type AgentType = golemAgent200Common.AgentType;
  export type RegisteredAgentType = golemAgent200Common.RegisteredAgentType;
  export type TypedAgentConfigValue = golemAgent200Common.TypedAgentConfigValue;
  /**
   * Creates a webhook that can be used to integrate with webhook driven apis.
   * When the created url is called with a post request, the provided promise-id is completed with the body of the post request.
   * Note the following behaviours:
   * * Only agents whoose agent types are _currently_ deployed via an http api are allowed to create a webhook. Calling this function while the agent
   *    is not deployed via an http api will trap.
   * * Only the agent type that created the promise is allowed to create a webhook for it. Using this host function
   *   from a different agent type will trap.
   */
  export type WebhookError =
  {
    tag: 'permission-denied'
  } |
  {
    tag: 'internal-error'
    val: string
  };
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
  } |
  /**
   * A keyed create found an agent that already has the key, or the agent at the ID
   * that a key derives has another key
   */
  {
    tag: 'conflict'
    val: string
  };
  /**
   * Options of `wasm-rpc.create`
   */
  export type CreateOptions = {
    /**
     * Creates the keyed agent of the constructor parameters, and fails with `conflict`
     * when it already exists. Otherwise the new agent has no key and a new ID.
     */
    keyed: boolean;
    /** Overrides of the agent type's local configuration values */
    agentConfig: TypedAgentConfigValue[];
    /** Environment variables added to the ones the manifest defines */
    env: [string, string][];
  };
  /**
   * Final identity allocated for one remote invocation. For an ephemeral
   * target, `agent-id` is the ID of the one-shot instance that ran it.
   */
  export type InvocationMetadata = {
    agentId: AgentId;
    invocationId: InvocationId;
  };
  /**
   * Result of an awaited invocation together with its final identity.
   */
  export type InvocationResultWithMetadata = {
    metadata: InvocationMetadata;
    result?: SchemaValueTree;
  };
  /**
   * Receipt returned when an invocation has been scheduled.
   */
  export type ScheduledInvocationReceipt = {
    metadata: InvocationMetadata;
  };
  /**
   * An agent created by `wasm-rpc.create`
   */
  export type Created = {
    client: WasmRpc;
    agentId: AgentId;
  };
  /**
   * Asynchronous invocation handle together with its final identity, when
   * preparation succeeded. Rejected calls report their error through the future.
   */
  export type AsyncInvocationWithMetadata = {
    metadata?: InvocationMetadata;
    future: FutureInvokeResult;
  };
  /**
   * Receipt for a scheduled invocation that can still be cancelled.
   */
  export type CancelableScheduledInvocationReceipt = {
    metadata: InvocationMetadata;
    cancellationToken: CancellationToken;
  };
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
  export type ConfigValueError =
  {
    tag: 'permission-denied'
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
