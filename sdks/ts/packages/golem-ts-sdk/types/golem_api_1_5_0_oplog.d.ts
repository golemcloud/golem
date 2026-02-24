/**
 * Host interface for enumerating and searching for agent oplogs
 */
declare module 'golem:api/oplog@1.5.0' {
  import * as golemApi150Context from 'golem:api/context@1.5.0';
  import * as golemApi150Host from 'golem:api/host@1.5.0';
  import * as golemCore150Types from 'golem:core/types@1.5.0';
  import * as wasiClocks023WallClock from 'wasi:clocks/wall-clock@0.2.3';
  /**
   * Enriches raw oplog entries into public oplog entries by resolving oplog payloads
   * and augmenting entries with component metadata.
   * @throws string
   */
  export function enrichOplogEntries(environmentId: EnvironmentId, agentId: AgentId, entries: [OplogIndex, OplogEntry][], componentRevision: ComponentRevision): PublicOplogEntry[];
  export class GetOplog {
    constructor(agentId: AgentId, start: OplogIndex);
    getNext(): PublicOplogEntry[] | undefined;
  }
  export class SearchOplog {
    constructor(agentId: AgentId, text: string);
    getNext(): [OplogIndex, PublicOplogEntry][] | undefined;
  }
  export type Datetime = wasiClocks023WallClock.Datetime;
  export type ValueAndType = golemCore150Types.ValueAndType;
  export type AccountId = golemCore150Types.AccountId;
  export type DataValue = golemCore150Types.DataValue;
  export type DataSchema = golemCore150Types.DataSchema;
  export type ComponentRevision = golemApi150Host.ComponentRevision;
  export type OplogIndex = golemApi150Host.OplogIndex;
  export type PersistenceLevel = golemApi150Host.PersistenceLevel;
  export type EnvironmentId = golemApi150Host.EnvironmentId;
  export type RetryPolicy = golemApi150Host.RetryPolicy;
  export type Uuid = golemApi150Host.Uuid;
  export type AgentId = golemApi150Host.AgentId;
  export type Snapshot = golemApi150Host.Snapshot;
  export type Attribute = golemApi150Context.Attribute;
  export type AttributeValue = golemApi150Context.AttributeValue;
  export type SpanId = golemApi150Context.SpanId;
  export type TraceId = golemApi150Context.TraceId;
  export type WrappedFunctionType = 
  /**
   * The side-effect reads from the agent's local state (for example local file system,
   * random generator, etc.)
   */
  {
    tag: 'read-local'
  } |
  /** The side-effect writes to the agent's local state (for example local file system) */
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
    name: string;
    version: string;
    parameters: [string, string][];
  };
  export type CreateParameters = {
    timestamp: Datetime;
    agentId: AgentId;
    componentRevision: ComponentRevision;
    args: string[];
    env: [string, string][];
    createdBy: AccountId;
    environmentId: EnvironmentId;
    parent?: AgentId;
    componentSize: bigint;
    initialTotalLinearMemorySize: bigint;
    initialActivePlugins: PluginInstallationDescription[];
    configVars: [string, string][];
  };
  export type HostCallParameters = {
    timestamp: Datetime;
    functionName: string;
    request: ValueAndType;
    response: ValueAndType;
    wrappedFunctionType: WrappedFunctionType;
  };
  export type LocalSpanData = {
    spanId: SpanId;
    start: Datetime;
    parent?: SpanId;
    /** Optionally an index of the invocation-context field within agent-invocation */
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
  export type ErrorParameters = {
    timestamp: Datetime;
    error: string;
    retryFrom: OplogIndex;
  };
  export type OplogRegion = {
    start: OplogIndex;
    end: OplogIndex;
  };
  export type JumpParameters = {
    timestamp: Datetime;
    jump: OplogRegion;
  };
  export type ChangeRetryPolicyParameters = {
    timestamp: Datetime;
    newPolicy: RetryPolicy;
  };
  export type EndAtomicRegionParameters = {
    timestamp: Datetime;
    beginIndex: OplogIndex;
  };
  export type EndRemoteWriteParameters = {
    timestamp: Datetime;
    beginIndex: OplogIndex;
  };
  export type TypedDataValue = {
    value: DataValue;
    schema: DataSchema;
  };
  export type AgentInitializationParameters = {
    idempotencyKey: string;
    constructorParameters: TypedDataValue;
    traceId: string;
    traceStates: string[];
    invocationContext: SpanData[][];
  };
  export type AgentMethodInvocationParameters = {
    idempotencyKey: string;
    methodName: string;
    functionInput: TypedDataValue;
    traceId: string;
    traceStates: string[];
    invocationContext: SpanData[][];
  };
  export type LoadSnapshotParameters = {
    snapshot: Snapshot;
  };
  export type ProcessOplogEntriesParameters = {
    idempotencyKey: string;
  };
  export type ManualUpdateParameters = {
    targetRevision: ComponentRevision;
  };
  export type AgentInvocation = 
  {
    tag: 'agent-initialization'
    val: AgentInitializationParameters
  } |
  {
    tag: 'agent-method-invocation'
    val: AgentMethodInvocationParameters
  } |
  {
    tag: 'save-snapshot'
  } |
  {
    tag: 'load-snapshot'
    val: LoadSnapshotParameters
  } |
  {
    tag: 'process-oplog-entries'
    val: ProcessOplogEntriesParameters
  } |
  {
    tag: 'manual-update'
    val: ManualUpdateParameters
  };
  export type AgentInvocationStartedParameters = {
    timestamp: Datetime;
    invocation: AgentInvocation;
  };
  export type AgentInvocationOutputParameters = {
    output: TypedDataValue;
  };
  export type FallibleResultParameters = {
    error?: string;
  };
  export type SaveSnapshotResultParameters = {
    snapshot: Snapshot;
  };
  export type AgentInvocationResult = 
  {
    tag: 'agent-initialization'
    val: AgentInvocationOutputParameters
  } |
  {
    tag: 'agent-method'
    val: AgentInvocationOutputParameters
  } |
  {
    tag: 'manual-update'
  } |
  {
    tag: 'load-snapshot'
    val: FallibleResultParameters
  } |
  {
    tag: 'save-snapshot'
    val: SaveSnapshotResultParameters
  } |
  {
    tag: 'process-oplog-entries'
    val: FallibleResultParameters
  };
  export type AgentInvocationFinishedParameters = {
    timestamp: Datetime;
    invocationResult: AgentInvocationResult;
    consumedFuel: bigint;
  };
  export type PendingAgentInvocationParameters = {
    timestamp: Datetime;
    invocation: AgentInvocation;
  };
  export type UpdateDescription = 
  /** Automatic update by replaying the oplog on the new version */
  {
    tag: 'auto-update'
  } |
  /** Custom update by loading a given snapshot on the new version */
  {
    tag: 'snapshot-based'
    val: Snapshot
  };
  export type PendingUpdateParameters = {
    timestamp: Datetime;
    targetRevision: ComponentRevision;
    updateDescription: UpdateDescription;
  };
  export type SuccessfulUpdateParameters = {
    timestamp: Datetime;
    targetRevision: ComponentRevision;
    newComponentSize: bigint;
    newActivePlugins: PluginInstallationDescription[];
  };
  export type FailedUpdateParameters = {
    timestamp: Datetime;
    targetRevision: ComponentRevision;
    details?: string;
  };
  export type GrowMemoryParameters = {
    timestamp: Datetime;
    delta: bigint;
  };
  export type AgentResourceId = bigint;
  export type CreateResourceParameters = {
    timestamp: Datetime;
    resourceId: AgentResourceId;
    name: string;
    owner: string;
  };
  export type DropResourceParameters = {
    timestamp: Datetime;
    resourceId: AgentResourceId;
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
    droppedRegion: OplogRegion;
  };
  export type CancelPendingInvocationParameters = {
    timestamp: Datetime;
    idempotencyKey: string;
  };
  export type StartSpanParameters = {
    timestamp: Datetime;
    spanId: SpanId;
    parent?: SpanId;
    linkedContextId?: SpanId;
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
  export type SnapshotParameters = {
    timestamp: Datetime;
    data: Uint8Array;
    mimeType: string;
  };
  export type Timestamp = {
    timestamp: Datetime;
  };
  export type OplogExternalPayload = {
    payloadId: Uuid;
    md5Hash: Uint8Array;
  };
  /**
   * Opaque oplog payload, which can either be serialized inline or stored externally
   */
  export type OplogPayload = 
  {
    tag: 'inline'
    val: Uint8Array
  } |
  {
    tag: 'external'
    val: OplogExternalPayload
  };
  /**
   * Describes the error that occurred in the agent
   */
  export type WorkerError = 
  {
    tag: 'unknown'
    val: string
  } |
  {
    tag: 'invalid-request'
    val: string
  } |
  {
    tag: 'stack-overflow'
  } |
  {
    tag: 'out-of-memory'
  } |
  {
    tag: 'exceeded-memory-limit'
  } |
  {
    tag: 'agent-error'
    val: string
  };
  export type RawCreateParameters = {
    timestamp: Datetime;
    workerId: AgentId;
    componentRevision: ComponentRevision;
    env: [string, string][];
    environmentId: EnvironmentId;
    createdBy: AccountId;
    parent?: AgentId;
    componentSize: bigint;
    initialTotalLinearMemorySize: bigint;
    initialActivePlugins: number[];
    wasiConfigVars: [string, string][];
    originalPhantomId?: Uuid;
  };
  export type RawHostCallParameters = {
    timestamp: Datetime;
    functionName: string;
    request: OplogPayload;
    response: OplogPayload;
    durableFunctionType: WrappedFunctionType;
  };
  export type RawAgentInvocationStartedParameters = {
    timestamp: Datetime;
    idempotencyKey: string;
    payload: OplogPayload;
    traceId: string;
    traceStates: string[];
    invocationContext: SpanData[];
  };
  export type RawAgentInvocationFinishedParameters = {
    timestamp: Datetime;
    result: OplogPayload;
    consumedFuel: bigint;
  };
  export type RawErrorParameters = {
    timestamp: Datetime;
    error: WorkerError;
    retryFrom: OplogIndex;
  };
  export type RawPendingAgentInvocationParameters = {
    timestamp: Datetime;
    idempotencyKey: string;
    payload: OplogPayload;
  };
  export type RawSnapshotBasedUpdate = {
    targetRevision: ComponentRevision;
    payload: OplogPayload;
    mimeType: string;
  };
  /**
   * Raw update description used in oplog entries
   */
  export type RawUpdateDescription = 
  /** Automatic update by replaying the oplog on the new version */
  {
    tag: 'automatic'
    val: ComponentRevision
  } |
  /** Custom update by loading a given snapshot on the new version */
  {
    tag: 'snapshot-based'
    val: RawSnapshotBasedUpdate
  };
  export type RawPendingUpdateParameters = {
    timestamp: Datetime;
    description: RawUpdateDescription;
  };
  export type RawSuccessfulUpdateParameters = {
    timestamp: Datetime;
    targetRevision: ComponentRevision;
    newComponentSize: bigint;
    newActivePlugins: number[];
  };
  export type ResourceTypeId = {
    name: string;
    owner: string;
  };
  export type RawCreateResourceParameters = {
    timestamp: Datetime;
    id: AgentResourceId;
    resourceTypeId: ResourceTypeId;
  };
  export type RawDropResourceParameters = {
    timestamp: Datetime;
    id: AgentResourceId;
    resourceTypeId: ResourceTypeId;
  };
  export type RawActivatePluginParameters = {
    timestamp: Datetime;
    pluginPriority: number;
  };
  export type RawDeactivatePluginParameters = {
    timestamp: Datetime;
    pluginPriority: number;
  };
  export type RawBeginRemoteTransactionParameters = {
    timestamp: Datetime;
    transactionId: string;
    originalBeginIndex?: OplogIndex;
  };
  export type RawSnapshotParameters = {
    timestamp: Datetime;
    data: OplogPayload;
    mimeType: string;
  };
  export type OplogEntry = 
  /** The initial agent oplog entry */
  {
    tag: 'create'
    val: RawCreateParameters
  } |
  /** The agent invoked a host function */
  {
    tag: 'host-call'
    val: RawHostCallParameters
  } |
  /** The agent has been invoked */
  {
    tag: 'agent-invocation-started'
    val: RawAgentInvocationStartedParameters
  } |
  /** The agent has completed an invocation */
  {
    tag: 'agent-invocation-finished'
    val: RawAgentInvocationFinishedParameters
  } |
  /** Agent suspended */
  {
    tag: 'suspend'
    val: Timestamp
  } |
  /** Agent failed */
  {
    tag: 'error'
    val: RawErrorParameters
  } |
  /**
   * Marker entry added when get-oplog-index is called from the agent, to make the jumping behavior
   * more predictable.
   */
  {
    tag: 'no-op'
    val: Timestamp
  } |
  /**
   * The agent needs to recover up to the given target oplog index and continue running from
   * the source oplog index from there
   * `jump` is an oplog region representing that from the end of that region we want to go back to the start and
   * ignore all recorded operations in between.
   */
  {
    tag: 'jump'
    val: JumpParameters
  } |
  /**
   * Indicates that the agent has been interrupted at this point.
   * Only used to recompute the agent's (cached) status, has no effect on execution.
   */
  {
    tag: 'interrupted'
    val: Timestamp
  } |
  /** Indicates that the agent has been exited using WASI's exit function. */
  {
    tag: 'exited'
    val: Timestamp
  } |
  /** Overrides the agent's retry policy */
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
    val: Timestamp
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
    val: Timestamp
  } |
  /** Marks the end of a remote write operation. Only used when idempotence mode is off. */
  {
    tag: 'end-remote-write'
    val: EndRemoteWriteParameters
  } |
  /** An invocation request arrived while the agent was busy */
  {
    tag: 'pending-agent-invocation'
    val: RawPendingAgentInvocationParameters
  } |
  /** An update request arrived and will be applied as soon the agent restarts */
  {
    tag: 'pending-update'
    val: RawPendingUpdateParameters
  } |
  /** An update was successfully applied */
  {
    tag: 'successful-update'
    val: RawSuccessfulUpdateParameters
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
    val: RawCreateResourceParameters
  } |
  /** Dropped a resource instance */
  {
    tag: 'drop-resource'
    val: RawDropResourceParameters
  } |
  /** The agent emitted a log message */
  {
    tag: 'log'
    val: LogParameters
  } |
  /** The agent has been restarted, forgetting all its history */
  {
    tag: 'restart'
    val: Timestamp
  } |
  /** Activates a plugin */
  {
    tag: 'activate-plugin'
    val: RawActivatePluginParameters
  } |
  /** Deactivates a plugin */
  {
    tag: 'deactivate-plugin'
    val: RawDeactivatePluginParameters
  } |
  /** Revert an agent to a previous state */
  {
    tag: 'revert'
    val: RevertParameters
  } |
  /** Cancel a pending invocation */
  {
    tag: 'cancel-pending-invocation'
    val: CancelPendingInvocationParameters
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
    val: RawBeginRemoteTransactionParameters
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
  } |
  /** A snapshot of the agent's state */
  {
    tag: 'snapshot'
    val: RawSnapshotParameters
  };
  export type PublicOplogEntry = 
  /** The initial agent oplog entry */
  {
    tag: 'create'
    val: CreateParameters
  } |
  /** The agent invoked a host function */
  {
    tag: 'host-call'
    val: HostCallParameters
  } |
  /** The agent has been invoked */
  {
    tag: 'agent-invocation-started'
    val: AgentInvocationStartedParameters
  } |
  /** The agent has completed an invocation */
  {
    tag: 'agent-invocation-finished'
    val: AgentInvocationFinishedParameters
  } |
  /** Agent suspended */
  {
    tag: 'suspend'
    val: Timestamp
  } |
  /** Agent failed */
  {
    tag: 'error'
    val: ErrorParameters
  } |
  /**
   * Marker entry added when get-oplog-index is called from the agent, to make the jumping behavior
   * more predictable.
   */
  {
    tag: 'no-op'
    val: Timestamp
  } |
  /**
   * The agent needs to recover up to the given target oplog index and continue running from
   * the source oplog index from there
   * `jump` is an oplog region representing that from the end of that region we want to go back to the start and
   * ignore all recorded operations in between.
   */
  {
    tag: 'jump'
    val: JumpParameters
  } |
  /**
   * Indicates that the agent has been interrupted at this point.
   * Only used to recompute the agent's (cached) status, has no effect on execution.
   */
  {
    tag: 'interrupted'
    val: Timestamp
  } |
  /** Indicates that the agent has been exited using WASI's exit function. */
  {
    tag: 'exited'
    val: Timestamp
  } |
  /** Overrides the agent's retry policy */
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
    val: Timestamp
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
    val: Timestamp
  } |
  /** Marks the end of a remote write operation. Only used when idempotence mode is off. */
  {
    tag: 'end-remote-write'
    val: EndRemoteWriteParameters
  } |
  /** An invocation request arrived while the agent was busy */
  {
    tag: 'pending-agent-invocation'
    val: PendingAgentInvocationParameters
  } |
  /** An update request arrived and will be applied as soon the agent restarts */
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
  /** The agent emitted a log message */
  {
    tag: 'log'
    val: LogParameters
  } |
  /** The agent's has been restarted, forgetting all its history */
  {
    tag: 'restart'
    val: Timestamp
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
  /** Revert an agent to a previous state */
  {
    tag: 'revert'
    val: RevertParameters
  } |
  /** Cancel a pending invocation */
  {
    tag: 'cancel-pending-invocation'
    val: CancelPendingInvocationParameters
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
  } |
  /** A snapshot of the worker's state */
  {
    tag: 'snapshot'
    val: SnapshotParameters
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
