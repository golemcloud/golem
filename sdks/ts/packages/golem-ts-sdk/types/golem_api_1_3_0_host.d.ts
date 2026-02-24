/**
 * The Golem host API provides low level access to Golem specific features such as promises and control over
 * the durability and transactional guarantees the executor provides.
 */
declare module 'golem:api/host@1.3.0' {
  import * as golemRpc022Types from 'golem:rpc/types@0.2.2';
  import * as wasiClocks023MonotonicClock from 'wasi:clocks/monotonic-clock@0.2.3';
  import * as wasiIo023Poll from 'wasi:io/poll@0.2.3';
  /**
   * Create a new promise
   */
  export function createPromise(): PromiseId;
  /**
   * Gets a handle to the result of the promise. Can only be called in the same agent that orignally created the promise.
   */
  export function getPromise(promiseId: PromiseId): GetPromiseResult;
  /**
   * Completes the given promise with the given payload. Returns true if the promise was completed, false
   * if the promise was already completed. The payload is passed to the agent that is awaiting the promise.
   */
  export function completePromise(promiseId: PromiseId, data: Uint8Array): boolean;
  /**
   * Returns the current position in the persistent op log
   */
  export function getOplogIndex(): OplogIndex;
  /**
   * Makes the current agent travel back in time and continue execution from the given position in the persistent
   * op log.
   */
  export function setOplogIndex(oplogIdx: OplogIndex): void;
  /**
   * Blocks the execution until the oplog has been written to at least the specified number of replicas,
   * or the maximum number of replicas if the requested number is higher.
   */
  export function oplogCommit(replicas: number): void;
  /**
   * Marks the beginning of an atomic operation.
   * In case of a failure within the region selected by `mark-begin-operation` and `mark-end-operation`
   * the whole region will be reexecuted on retry.
   * The end of the region is when `mark-end-operation` is called with the returned oplog-index.
   */
  export function markBeginOperation(): OplogIndex;
  /**
   * Commits this atomic operation. After `mark-end-operation` is called for a given index, further calls
   * with the same parameter will do nothing.
   */
  export function markEndOperation(begin: OplogIndex): void;
  /**
   * Gets the current retry policy associated with the agent
   */
  export function getRetryPolicy(): RetryPolicy;
  /**
   * Overrides the current retry policy associated with the agent. Following this call, `get-retry-policy` will return the
   * new retry policy.
   */
  export function setRetryPolicy(newRetryPolicy: RetryPolicy): void;
  /**
   * Gets the agent's current persistence level.
   */
  export function getOplogPersistenceLevel(): PersistenceLevel;
  /**
   * Sets the agent's current persistence level. This can increase the performance of execution in cases where durable
   * execution is not required.
   */
  export function setOplogPersistenceLevel(newPersistenceLevel: PersistenceLevel): void;
  /**
   * Gets the current idempotence mode. See `set-idempotence-mode` for details.
   */
  export function getIdempotenceMode(): boolean;
  /**
   * Sets the current idempotence mode. The default is true.
   * True means side-effects are treated idempotent and Golem guarantees at-least-once semantics.
   * In case of false the executor provides at-most-once semantics, failing the agent in case it is
   * not known if the side effect was already executed.
   */
  export function setIdempotenceMode(idempotent: boolean): void;
  /**
   * Generates an idempotency key. This operation will never be replayed —
   * i.e. not only is this key generated, but it is persisted and committed, such that the key can be used in third-party systems (e.g. payment processing)
   * to introduce idempotence.
   */
  export function generateIdempotencyKey(): Uuid;
  /**
   * Initiates an update attempt for the given agent. The function returns immediately once the request has been processed,
   * not waiting for the agent to get updated.
   */
  export function updateAgent(agentId: AgentId, targetRevision: ComponentRevision, mode: UpdateMode): void;
  /**
   * Get the current agent's metadata
   */
  export function getSelfMetadata(): AgentMetadata;
  /**
   * Get agent metadata
   */
  export function getAgentMetadata(agentId: AgentId): AgentMetadata | undefined;
  /**
   * Fork an agent to another agent at a given oplog index
   */
  export function forkAgent(sourceAgentId: AgentId, targetAgentId: AgentId, oplogIdxCutOff: OplogIndex): void;
  /**
   * Revert an agent to a previous state
   */
  export function revertAgent(agentId: AgentId, revertTarget: RevertAgentTarget): void;
  /**
   * Get the component-id for a given component reference.
   * Returns none when no component with the specified reference exists.
   * The syntax of the component reference is implementation dependent.
   * Golem OSS: "{component_name}"
   * Golem Cloud:
   *     1: "{component_name}" -> will resolve in current account and project
   *     2: "{project_name}/{component_name}" -> will resolve in current account
   *     3: "{account_id}/{project_name}/{component_name}"
   */
  export function resolveComponentId(componentReference: string): ComponentId | undefined;
  /**
   * Get the agent-id for a given component and agent name.
   * Returns none when no component for the specified reference exists.
   */
  export function resolveAgentId(componentReference: string, agentName: string): AgentId | undefined;
  /**
   * Get the agent-id for a given component and agent-name.
   * Returns none when no component for the specified component-reference or no agent with the specified agent-name exists.
   */
  export function resolveAgentIdStrict(componentReference: string, agentName: string): AgentId | undefined;
  /**
   * Forks the current agent at the current execution point. The new agent gets the same base agent ID but
   * with a new unique phantom ID. The phantom ID of the forked agent is returned in `fork-result` on
   * both sides. The newly created agent continues running from the same point, but the return value is
   * going to be different in this agent and the forked agent.
   */
  export function fork(): ForkResult;
  export class GetAgents {
    /**
     * Creates an agent enumeration request. It is going to enumerate all agents of all the agent types
     * defined in `component-id`, filtered by the conditions given by `filter`. If `precise` is true,
     * the server will calculate the most recent state of all the returned agents, otherwise the returned
     * metadata will be not guaranteed to be up-to-date.
     */
    constructor(componentId: ComponentId, filter: AgentAnyFilter | undefined, precise: boolean);
    /**
     * Retrieves the next batch of agent metadata.
     */
    getNext(): AgentMetadata[] | undefined;
  }
  export class GetPromiseResult {
    /**
     * Returns a pollable that can be used to wait for the promise to become ready.j
     */
    subscribe(): Pollable;
    /**
     * Poll the result of the promise, returning none if it is not yet ready.
     */
    get(): Uint8Array | undefined;
  }
  export type Duration = wasiClocks023MonotonicClock.Duration;
  export type ComponentId = golemRpc022Types.ComponentId;
  export type Uuid = golemRpc022Types.Uuid;
  export type ValueAndType = golemRpc022Types.ValueAndType;
  export type AgentId = golemRpc022Types.AgentId;
  export type PromiseId = golemRpc022Types.PromiseId;
  export type OplogIndex = golemRpc022Types.OplogIndex;
  export type Pollable = wasiIo023Poll.Pollable;
  /**
   * Represents a Golem component's version
   */
  export type ComponentRevision = bigint;
  /**
   * Represents a Golem environment
   */
  export type EnvironmentId = {
    uuid: Uuid;
  };
  /**
   * Configures how the executor retries failures
   */
  export type RetryPolicy = {
    /** The maximum number of retries before the agent becomes permanently failed */
    maxAttempts: number;
    /** The minimum delay between retries (applied to the first retry) */
    minDelay: Duration;
    /** The maximum delay between retries */
    maxDelay: Duration;
    /** Multiplier applied to the delay on each retry to implement exponential backoff */
    multiplier: number;
    /** The maximum amount of jitter to add to the delay */
    maxJitterFactor?: number;
  };
  /**
   * Configurable persistence level for agents
   */
  export type PersistenceLevel = 
  {
    tag: 'persist-nothing'
  } |
  {
    tag: 'persist-remote-side-effects'
  } |
  {
    tag: 'smart'
  };
  /**
   * Describes how to update an agent to a different component version
   */
  export type UpdateMode = "automatic" | "snapshot-based";
  /**
   * Operators used in filtering enumerated agents
   */
  export type FilterComparator = "equal" | "not-equal" | "greater-equal" | "greater" | "less-equal" | "less";
  /**
   * Operators used on strings in filtering enumerated agents
   */
  export type StringFilterComparator = "equal" | "not-equal" | "like" | "not-like" | "starts-with";
  /**
   * The current status of an agent
   */
  export type AgentStatus = "running" | "idle" | "suspended" | "interrupted" | "retrying" | "failed" | "exited";
  /**
   * Describes a filter condition on agent IDs when enumerating agents
   */
  export type AgentNameFilter = {
    comparator: StringFilterComparator;
    value: string;
  };
  /**
   * Describes a filter condition on the agent status when enumerating agents
   */
  export type AgentStatusFilter = {
    comparator: FilterComparator;
    value: AgentStatus;
  };
  /**
   * Describes a filter condition on the component version when enumerating agents
   */
  export type AgentVersionFilter = {
    comparator: FilterComparator;
    value: bigint;
  };
  /**
   * Describes a filter condition on the agent's creation time when enumerating agents
   */
  export type AgentCreatedAtFilter = {
    comparator: FilterComparator;
    value: bigint;
  };
  /**
   * Describes a filter condition on the agent's environment variables when enumerating agents
   */
  export type AgentEnvFilter = {
    name: string;
    comparator: StringFilterComparator;
    value: string;
  };
  /**
   * Describes a filter condition on the agent's configuration variables when enumerating agents
   */
  export type AgentConfigVarsFilter = {
    name: string;
    comparator: StringFilterComparator;
    value: string;
  };
  /**
   * Describes one filter condition for enumerating agents
   */
  export type AgentPropertyFilter = 
  {
    tag: 'name'
    val: AgentNameFilter
  } |
  {
    tag: 'status'
    val: AgentStatusFilter
  } |
  {
    tag: 'version'
    val: AgentVersionFilter
  } |
  {
    tag: 'created-at'
    val: AgentCreatedAtFilter
  } |
  {
    tag: 'env'
    val: AgentEnvFilter
  } |
  {
    tag: 'wasi-config-vars'
    val: AgentConfigVarsFilter
  };
  /**
   * Combines multiple filter conditions with an `AND` relationship for enumerating agents
   */
  export type AgentAllFilter = {
    filters: AgentPropertyFilter[];
  };
  /**
   * Combines multiple groups of filter conditions with an `OR` relationship for enumerating agents
   */
  export type AgentAnyFilter = {
    filters: AgentAllFilter[];
  };
  /**
   * Metadata about an agent
   */
  export type AgentMetadata = {
    /** The agent ID, consists of the component ID, agent type and agent parameters */
    agentId: AgentId;
    /** Command line arguments seen by the agent */
    args: string[];
    /** Environment variables seen by the agent */
    env: [string, string][];
    /** Configuration variables seen by the agent */
    configVars: [string, string][];
    /** The current agent status */
    status: AgentStatus;
    /** The component version the agent is running with */
    componentRevision: bigint;
    /** The agent's current retry count */
    retryCount: bigint;
  };
  /**
   * Target parameter for the `revert-agent` operation
   */
  export type RevertAgentTarget = 
  /** Revert to a specific oplog index. The given index will be the last one to be kept. */
  {
    tag: 'revert-to-oplog-index'
    val: OplogIndex
  } |
  /** Revert the last N invocations. */
  {
    tag: 'revert-last-invocations'
    val: bigint
  };
  /**
   * Details about the fork result
   */
  export type ForkDetails = {
    forkedPhantomId: Uuid;
  };
  /**
   * Indicates which agent the code is running on after `fork`.
   * The parameter contains details about the fork result, such as the phantom-ID of the newly
   * created agent.
   */
  export type ForkResult = 
  /** The original agent that called `fork` */
  {
    tag: 'original'
    val: ForkDetails
  } |
  /** The new agent */
  {
    tag: 'forked'
    val: ForkDetails
  };
  /**
   * Snapshot payload
   */
  export type Snapshot = {
    data: Uint8Array;
    mimeType: string;
  };
}
