/**
 * The Golem host API provides low level access to Golem specific features such as promises and control over
 * the durability and transactional guarantees the executor provides.
 */
declare module 'golem:api/host@1.1.7' {
  import * as golemRpc022Types from 'golem:rpc/types@0.2.2';
  import * as wasiClocks023MonotonicClock from 'wasi:clocks/monotonic-clock@0.2.3';
  /**
   * Create a new promise
   */
  export function createPromise(): PromiseId;
  /**
   * Suspends execution until the given promise gets completed, and returns the payload passed to
   * the promise completion.
   */
  export function awaitPromise(promiseId: PromiseId): Uint8Array;
  /**
   * Checks whether the given promise is completed. If not, it returns None. If the promise is completed,
   * it returns the payload passed to the promise completion.
   */
  export function pollPromise(promiseId: PromiseId): Uint8Array | undefined;
  /**
   * Completes the given promise with the given payload. Returns true if the promise was completed, false
   * if the promise was already completed. The payload is passed to the worker that is awaiting the promise.
   */
  export function completePromise(promiseId: PromiseId, data: Uint8Array): boolean;
  /**
   * Deletes the given promise
   */
  export function deletePromise(promiseId: PromiseId): void;
  /**
   * Returns the current position in the persistent op log
   */
  export function getOplogIndex(): OplogIndex;
  /**
   * Makes the current worker travel back in time and continue execution from the given position in the persistent
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
   * Gets the current retry policy associated with the worker
   */
  export function getRetryPolicy(): RetryPolicy;
  /**
   * Overrides the current retry policy associated with the worker. Following this call, `get-retry-policy` will return the
   * new retry policy.
   */
  export function setRetryPolicy(newRetryPolicy: RetryPolicy): void;
  /**
   * Gets the worker's current persistence level.
   */
  export function getOplogPersistenceLevel(): PersistenceLevel;
  /**
   * Sets the worker's current persistence level. This can increase the performance of execution in cases where durable
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
   * In case of false the executor provides at-most-once semantics, failing the worker in case it is
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
   * Initiates an update attempt for the given worker. The function returns immediately once the request has been processed,
   * not waiting for the worker to get updated.
   */
  export function updateWorker(workerId: WorkerId, targetVersion: ComponentVersion, mode: UpdateMode): void;
  /**
   * Get current worker metadata
   */
  export function getSelfMetadata(): WorkerMetadata;
  /**
   * Get worker metadata
   */
  export function getWorkerMetadata(workerId: WorkerId): WorkerMetadata | undefined;
  /**
   * Fork a worker to another worker at a given oplog index
   */
  export function forkWorker(sourceWorkerId: WorkerId, targetWorkerId: WorkerId, oplogIdxCutOff: OplogIndex): void;
  /**
   * Revert a worker to a previous state
   */
  export function revertWorker(workerId: WorkerId, revertTarget: RevertWorkerTarget): void;
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
   * Get the worker-id for a given component and worker name.
   * Returns none when no component for the specified reference exists.
   */
  export function resolveWorkerId(componentReference: string, workerName: string): WorkerId | undefined;
  /**
   * Get the worker-id for a given component and worker name.
   * Returns none when no component for the specified component-reference or no worker with the specified worker-name exists.
   */
  export function resolveWorkerIdStrict(componentReference: string, workerName: string): WorkerId | undefined;
  /**
   * Forks the current worker at the current execution point. The new worker gets the `new-name` worker name,
   * and this worker continues running as well. The return value is going to be different in this worker and
   * the forked worker.
   */
  export function fork(newName: string): ForkResult;
  export class GetWorkers {
    constructor(componentId: ComponentId, filter: WorkerAnyFilter | undefined, precise: boolean);
    getNext(): WorkerMetadata[] | undefined;
  }
  export type Duration = wasiClocks023MonotonicClock.Duration;
  export type ComponentId = golemRpc022Types.ComponentId;
  export type Uuid = golemRpc022Types.Uuid;
  export type ValueAndType = golemRpc022Types.ValueAndType;
  export type WorkerId = golemRpc022Types.WorkerId;
  /**
   * An index into the persistent log storing all performed operations of a worker
   */
  export type OplogIndex = bigint;
  /**
   * A promise ID is a value that can be passed to an external Golem API to complete that promise
   * from an arbitrary external source, while Golem workers can await for this completion.
   */
  export type PromiseId = {
    workerId: WorkerId;
    oplogIdx: OplogIndex;
  };
  /**
   * Represents a Golem component's version
   */
  export type ComponentVersion = bigint;
  /**
   * Represents a Golem Cloud account
   */
  export type AccountId = {
    value: string;
  };
  /**
   * Represents a Golem project
   */
  export type ProjectId = {
    uuid: Uuid;
  };
  /**
   * Configures how the executor retries failures
   */
  export type RetryPolicy = {
    maxAttempts: number;
    minDelay: Duration;
    maxDelay: Duration;
    multiplier: number;
    maxJitterFactor: number | undefined;
  };
  /**
   * Configurable persistence level for workers
   */
  export type PersistenceLevel = {
    tag: 'persist-nothing'
  } |
  {
    tag: 'persist-remote-side-effects'
  } |
  {
    tag: 'smart'
  };
  /**
   * Describes how to update a worker to a different component version
   */
  export type UpdateMode = "automatic" | "snapshot-based";
  export type FilterComparator = "equal" | "not-equal" | "greater-equal" | "greater" | "less-equal" | "less";
  export type StringFilterComparator = "equal" | "not-equal" | "like" | "not-like" | "starts-with";
  export type WorkerStatus = "running" | "idle" | "suspended" | "interrupted" | "retrying" | "failed" | "exited";
  export type WorkerNameFilter = {
    comparator: StringFilterComparator;
    value: string;
  };
  export type WorkerStatusFilter = {
    comparator: FilterComparator;
    value: WorkerStatus;
  };
  export type WorkerVersionFilter = {
    comparator: FilterComparator;
    value: bigint;
  };
  export type WorkerCreatedAtFilter = {
    comparator: FilterComparator;
    value: bigint;
  };
  export type WorkerEnvFilter = {
    name: string;
    comparator: StringFilterComparator;
    value: string;
  };
  export type WorkerWasiConfigVarsFilter = {
    name: string;
    comparator: StringFilterComparator;
    value: string;
  };
  export type WorkerPropertyFilter = {
    tag: 'name'
    val: WorkerNameFilter
  } |
  {
    tag: 'status'
    val: WorkerStatusFilter
  } |
  {
    tag: 'version'
    val: WorkerVersionFilter
  } |
  {
    tag: 'created-at'
    val: WorkerCreatedAtFilter
  } |
  {
    tag: 'env'
    val: WorkerEnvFilter
  } |
  {
    tag: 'wasi-config-vars'
    val: WorkerWasiConfigVarsFilter
  };
  export type WorkerAllFilter = {
    filters: WorkerPropertyFilter[];
  };
  export type WorkerAnyFilter = {
    filters: WorkerAllFilter[];
  };
  export type WorkerMetadata = {
    workerId: WorkerId;
    args: string[];
    env: [string, string][];
    wasiConfigVars: [string, string][];
    status: WorkerStatus;
    componentVersion: bigint;
    retryCount: bigint;
  };
  /**
   * Target parameter for the `revert-worker` operation
   */
  export type RevertWorkerTarget = {
    tag: 'revert-to-oplog-index'
    val: OplogIndex
  } |
  {
    tag: 'revert-last-invocations'
    val: bigint
  };
  /**
   * Indicates which worker the code is running on after `fork`
   */
  export type ForkResult = "original" | "forked";
}
