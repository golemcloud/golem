/**
 * Host interface for enumerating and searching for worker oplogs
 */
declare module 'golem:api/oplog@1.1.7' {
  import * as golemApi117Context from 'golem:api/context@1.1.7';
  import * as golemApi117Host from 'golem:api/host@1.1.7';
  import * as golemRpc022Types from 'golem:rpc/types@0.2.2';
  import * as wasiClocks023WallClock from 'wasi:clocks/wall-clock@0.2.3';
  export class GetOplog {
    constructor(workerId: WorkerId, start: OplogIndex);
    getNext(): OplogEntry[] | undefined;
  }
  export class SearchOplog {
    constructor(workerId: WorkerId, text: string);
    getNext(): [OplogIndex, OplogEntry][] | undefined;
  }
  export type Datetime = wasiClocks023WallClock.Datetime;
  export type WitValue = golemRpc022Types.WitValue;
  export type AccountId = golemApi117Host.AccountId;
  export type ComponentVersion = golemApi117Host.ComponentVersion;
  export type OplogIndex = golemApi117Host.OplogIndex;
  export type PersistenceLevel = golemApi117Host.PersistenceLevel;
  export type ProjectId = golemApi117Host.ProjectId;
  export type RetryPolicy = golemApi117Host.RetryPolicy;
  export type Uuid = golemApi117Host.Uuid;
  export type WorkerId = golemApi117Host.WorkerId;
  export type Attribute = golemApi117Context.Attribute;
  export type AttributeValue = golemApi117Context.AttributeValue;
  export type SpanId = golemApi117Context.SpanId;
  export type TraceId = golemApi117Context.TraceId;
  export type WrappedFunctionType = 
  /**
   * The side-effect reads from the worker's local state (for example local file system,
   * random generator, etc.)
   */
  {
    tag: 'read-local'
  } |
  /** The side-effect writes to the worker's local state (for example local file system) */
  {
    tag: 'write-local'
  } |
  /** The side-effect reads from external state (for example a key-value store) */
  {
    tag: 'read-remote'
  } |
  /** The side-effect manipulates external state (for example an RPC call) */
  {
    tag: 'write-remote'
  } |
  /**
   * The side-effect manipulates external state through multiple invoked functions (for example
   * a HTTP request where reading the response involves multiple host function calls)
   * On the first invocation of the batch, the parameter should be `None` - this triggers
   * writing a `BeginRemoteWrite` entry in the oplog. Followup invocations should contain
   * this entry's index as the parameter. In batched remote writes it is the caller's responsibility
   * to manually write an `EndRemoteWrite` entry (using `end_function`) when the operation is completed.
   */
  {
    tag: 'write-remote-batched'
    val: OplogIndex | undefined
  } |
  {
    tag: 'write-remote-transaction'
    val: OplogIndex | undefined
  };
  export type PluginInstallationDescription = {
    installationId: Uuid;
    name: string;
    version: string;
    parameters: [string, string][];
  };
  export type CreateParameters = {
    timestamp: Datetime;
    workerId: WorkerId;
    componentVersion: ComponentVersion;
    args: string[];
    env: [string, string][];
    createdBy: AccountId;
    projectId: ProjectId;
    parent?: WorkerId;
    componentSize: bigint;
    initialTotalLinearMemorySize: bigint;
    initialActivePlugins: PluginInstallationDescription[];
  };
  export type ImportedFunctionInvokedParameters = {
    timestamp: Datetime;
    functionName: string;
    request: WitValue;
    response: WitValue;
    wrappedFunctionType: WrappedFunctionType;
  };
  export type LocalSpanData = {
    spanId: SpanId;
    start: Datetime;
    parent?: SpanId;
    /** Optionally an index of the exported-function-invoked-parameters's invocation-context field */
    linkedContext?: bigint;
    attributes: Attribute[];
    inherited: boolean;
  };
  export type ExternalSpanData = {
    spanId: SpanId;
  };
  export type SpanData = 
  {
    tag: 'local-span'
    val: LocalSpanData
  } |
  {
    tag: 'external-span'
    val: ExternalSpanData
  };
  export type ExportedFunctionInvokedParameters = {
    timestamp: Datetime;
    functionName: string;
    request: WitValue[];
    idempotencyKey: string;
    traceId: TraceId;
    traceStates: string[];
    /**
     * The first one is the invocation context stack associated with the exported function invocation,
     * and further stacks can be added that are referenced by the `linked-context` field of `local-span-data`
     */
    invocationContext: SpanData[][];
  };
  export type ExportedFunctionCompletedParameters = {
    timestamp: Datetime;
    response?: WitValue;
    consumedFuel: bigint;
  };
  export type ErrorParameters = {
    timestamp: Datetime;
    error: string;
  };
  export type JumpParameters = {
    timestamp: Datetime;
    start: OplogIndex;
    end: OplogIndex;
  };
  export type ChangeRetryPolicyParameters = {
    timestamp: Datetime;
    retryPolicy: RetryPolicy;
  };
  export type EndAtomicRegionParameters = {
    timestamp: Datetime;
    beginIndex: OplogIndex;
  };
  export type EndRemoteWriteParameters = {
    timestamp: Datetime;
    beginIndex: OplogIndex;
  };
  export type ExportedFunctionInvocationParameters = {
    idempotencyKey: string;
    functionName: string;
    input?: WitValue[];
  };
  export type WorkerInvocation = 
  {
    tag: 'exported-function'
    val: ExportedFunctionInvocationParameters
  } |
  {
    tag: 'manual-update'
    val: ComponentVersion
  };
  export type PendingWorkerInvocationParameters = {
    timestamp: Datetime;
    invocation: WorkerInvocation;
  };
  export type UpdateDescription = 
  /** Automatic update by replaying the oplog on the new version */
  {
    tag: 'auto-update'
  } |
  /** Custom update by loading a given snapshot on the new version */
  {
    tag: 'snapshot-based'
    val: Uint8Array
  };
  export type PendingUpdateParameters = {
    timestamp: Datetime;
    targetVersion: ComponentVersion;
    updateDescription: UpdateDescription;
  };
  export type SuccessfulUpdateParameters = {
    timestamp: Datetime;
    targetVersion: ComponentVersion;
    newComponentSize: bigint;
    newActivePlugins: PluginInstallationDescription[];
  };
  export type FailedUpdateParameters = {
    timestamp: Datetime;
    targetVersion: ComponentVersion;
    details?: string;
  };
  export type GrowMemoryParameters = {
    timestamp: Datetime;
    delta: bigint;
  };
  export type WorkerResourceId = bigint;
  export type CreateResourceParameters = {
    timestamp: Datetime;
    resourceId: WorkerResourceId;
    name: string;
    owner: string;
  };
  export type DropResourceParameters = {
    timestamp: Datetime;
    resourceId: WorkerResourceId;
    name: string;
    owner: string;
  };
  export type LogLevel = "stdout" | "stderr" | "trace" | "debug" | "info" | "warn" | "error" | "critical";
  export type LogParameters = {
    timestamp: Datetime;
    level: LogLevel;
    context: string;
    message: string;
  };
  export type ActivatePluginParameters = {
    timestamp: Datetime;
    plugin: PluginInstallationDescription;
  };
  export type DeactivatePluginParameters = {
    timestamp: Datetime;
    plugin: PluginInstallationDescription;
  };
  export type RevertParameters = {
    timestamp: Datetime;
    start: OplogIndex;
    end: OplogIndex;
  };
  export type CancelInvocationParameters = {
    timestamp: Datetime;
    idempotencyKey: string;
  };
  export type StartSpanParameters = {
    timestamp: Datetime;
    spanId: SpanId;
    parent?: SpanId;
    linkedContext?: SpanId;
    attributes: Attribute[];
  };
  export type FinishSpanParameters = {
    timestamp: Datetime;
    spanId: SpanId;
  };
  export type SetSpanAttributeParameters = {
    timestamp: Datetime;
    spanId: SpanId;
    key: string;
    value: AttributeValue;
  };
  export type ChangePersistenceLevelParameters = {
    timestamp: Datetime;
    persistenceLevel: PersistenceLevel;
  };
  export type BeginRemoteTransactionParameters = {
    timestamp: Datetime;
    transactionId: string;
  };
  export type RemoteTransactionParameters = {
    timestamp: Datetime;
    beginIndex: OplogIndex;
  };
  export type OplogEntry = 
  /** The initial worker oplog entry */
  {
    tag: 'create'
    val: CreateParameters
  } |
  /** The worker invoked a host function */
  {
    tag: 'imported-function-invoked'
    val: ImportedFunctionInvokedParameters
  } |
  /** The worker has been invoked */
  {
    tag: 'exported-function-invoked'
    val: ExportedFunctionInvokedParameters
  } |
  /** The worker has completed an invocation */
  {
    tag: 'exported-function-completed'
    val: ExportedFunctionCompletedParameters
  } |
  /** Worker suspended */
  {
    tag: 'suspend'
    val: Datetime
  } |
  /** Worker failed */
  {
    tag: 'error'
    val: ErrorParameters
  } |
  /**
   * Marker entry added when get-oplog-index is called from the worker, to make the jumping behavior
   * more predictable.
   */
  {
    tag: 'no-op'
    val: Datetime
  } |
  /**
   * The worker needs to recover up to the given target oplog index and continue running from
   * the source oplog index from there
   * `jump` is an oplog region representing that from the end of that region we want to go back to the start and
   * ignore all recorded operations in between.
   */
  {
    tag: 'jump'
    val: JumpParameters
  } |
  /**
   * Indicates that the worker has been interrupted at this point.
   * Only used to recompute the worker's (cached) status, has no effect on execution.
   */
  {
    tag: 'interrupted'
    val: Datetime
  } |
  /** Indicates that the worker has been exited using WASI's exit function. */
  {
    tag: 'exited'
    val: Datetime
  } |
  /** Overrides the worker's retry policy */
  {
    tag: 'change-retry-policy'
    val: ChangeRetryPolicyParameters
  } |
  /**
   * Begins an atomic region. All oplog entries after `BeginAtomicRegion` are to be ignored during
   * recovery except if there is a corresponding `EndAtomicRegion` entry.
   */
  {
    tag: 'begin-atomic-region'
    val: Datetime
  } |
  /**
   * Ends an atomic region. All oplog entries between the corresponding `BeginAtomicRegion` and this
   * entry are to be considered during recovery, and the begin/end markers can be removed during oplog
   * compaction.
   */
  {
    tag: 'end-atomic-region'
    val: EndAtomicRegionParameters
  } |
  /**
   * Begins a remote write operation. Only used when idempotence mode is off. In this case each
   * remote write must be surrounded by a `BeginRemoteWrite` and `EndRemoteWrite` log pair and
   * unfinished remote writes cannot be recovered.
   */
  {
    tag: 'begin-remote-write'
    val: Datetime
  } |
  /** Marks the end of a remote write operation. Only used when idempotence mode is off. */
  {
    tag: 'end-remote-write'
    val: EndRemoteWriteParameters
  } |
  /** An invocation request arrived while the worker was busy */
  {
    tag: 'pending-worker-invocation'
    val: PendingWorkerInvocationParameters
  } |
  /** An update request arrived and will be applied as soon the worker restarts */
  {
    tag: 'pending-update'
    val: PendingUpdateParameters
  } |
  /** An update was successfully applied */
  {
    tag: 'successful-update'
    val: SuccessfulUpdateParameters
  } |
  /** An update failed to be applied */
  {
    tag: 'failed-update'
    val: FailedUpdateParameters
  } |
  /** Increased total linear memory size */
  {
    tag: 'grow-memory'
    val: GrowMemoryParameters
  } |
  /** Created a resource instance */
  {
    tag: 'create-resource'
    val: CreateResourceParameters
  } |
  /** Dropped a resource instance */
  {
    tag: 'drop-resource'
    val: DropResourceParameters
  } |
  /** The worker emitted a log message */
  {
    tag: 'log'
    val: LogParameters
  } |
  /** The worker's has been restarted, forgetting all its history */
  {
    tag: 'restart'
    val: Datetime
  } |
  /** Activates a plugin */
  {
    tag: 'activate-plugin'
    val: ActivatePluginParameters
  } |
  /** Deactivates a plugin */
  {
    tag: 'deactivate-plugin'
    val: DeactivatePluginParameters
  } |
  /** Revert a worker to a previous state */
  {
    tag: 'revert'
    val: RevertParameters
  } |
  /** Cancel a pending invocation */
  {
    tag: 'cancel-invocation'
    val: CancelInvocationParameters
  } |
  /** Start a new span in the invocation context */
  {
    tag: 'start-span'
    val: StartSpanParameters
  } |
  /** Finish an open span in the invocation context */
  {
    tag: 'finish-span'
    val: FinishSpanParameters
  } |
  /** Set an attribute on an open span in the invocation context */
  {
    tag: 'set-span-attribute'
    val: SetSpanAttributeParameters
  } |
  /** Change the current persistence level */
  {
    tag: 'change-persistence-level'
    val: ChangePersistenceLevelParameters
  } |
  /** Begins a transaction operation */
  {
    tag: 'begin-remote-transaction'
    val: BeginRemoteTransactionParameters
  } |
  /** Pre-Commit of the transaction, indicating that the transaction will be committed */
  {
    tag: 'pre-commit-remote-transaction'
    val: RemoteTransactionParameters
  } |
  /** Pre-Rollback of the transaction, indicating that the transaction will be rolled back */
  {
    tag: 'pre-rollback-remote-transaction'
    val: RemoteTransactionParameters
  } |
  /** Committed transaction operation, indicating that the transaction was committed */
  {
    tag: 'committed-remote-transaction'
    val: RemoteTransactionParameters
  } |
  /** Rolled back transaction operation, indicating that the transaction was rolled back */
  {
    tag: 'rolled-back-remote-transaction'
    val: RemoteTransactionParameters
  };
}
