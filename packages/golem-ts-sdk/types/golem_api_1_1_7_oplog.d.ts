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
  export type WrappedFunctionType = {
    tag: 'read-local'
  } |
  {
    tag: 'write-local'
  } |
  {
    tag: 'read-remote'
  } |
  {
    tag: 'write-remote'
  } |
  {
    tag: 'write-remote-batched'
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
    linkedContext?: bigint;
    attributes: Attribute[];
    inherited: boolean;
  };
  export type ExternalSpanData = {
    spanId: SpanId;
  };
  export type SpanData = {
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
  export type WorkerInvocation = {
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
  export type UpdateDescription = {
    tag: 'auto-update'
  } |
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
  export type OplogEntry = {
    tag: 'create'
    val: CreateParameters
  } |
  {
    tag: 'imported-function-invoked'
    val: ImportedFunctionInvokedParameters
  } |
  {
    tag: 'exported-function-invoked'
    val: ExportedFunctionInvokedParameters
  } |
  {
    tag: 'exported-function-completed'
    val: ExportedFunctionCompletedParameters
  } |
  {
    tag: 'suspend'
    val: Datetime
  } |
  {
    tag: 'error'
    val: ErrorParameters
  } |
  {
    tag: 'no-op'
    val: Datetime
  } |
  {
    tag: 'jump'
    val: JumpParameters
  } |
  {
    tag: 'interrupted'
    val: Datetime
  } |
  {
    tag: 'exited'
    val: Datetime
  } |
  {
    tag: 'change-retry-policy'
    val: ChangeRetryPolicyParameters
  } |
  {
    tag: 'begin-atomic-region'
    val: Datetime
  } |
  {
    tag: 'end-atomic-region'
    val: EndAtomicRegionParameters
  } |
  {
    tag: 'begin-remote-write'
    val: Datetime
  } |
  {
    tag: 'end-remote-write'
    val: EndRemoteWriteParameters
  } |
  {
    tag: 'pending-worker-invocation'
    val: PendingWorkerInvocationParameters
  } |
  {
    tag: 'pending-update'
    val: PendingUpdateParameters
  } |
  {
    tag: 'successful-update'
    val: SuccessfulUpdateParameters
  } |
  {
    tag: 'failed-update'
    val: FailedUpdateParameters
  } |
  {
    tag: 'grow-memory'
    val: GrowMemoryParameters
  } |
  {
    tag: 'create-resource'
    val: CreateResourceParameters
  } |
  {
    tag: 'drop-resource'
    val: DropResourceParameters
  } |
  {
    tag: 'log'
    val: LogParameters
  } |
  {
    tag: 'restart'
    val: Datetime
  } |
  {
    tag: 'activate-plugin'
    val: ActivatePluginParameters
  } |
  {
    tag: 'deactivate-plugin'
    val: DeactivatePluginParameters
  } |
  {
    tag: 'revert'
    val: RevertParameters
  } |
  {
    tag: 'cancel-invocation'
    val: CancelInvocationParameters
  } |
  {
    tag: 'start-span'
    val: StartSpanParameters
  } |
  {
    tag: 'finish-span'
    val: FinishSpanParameters
  } |
  {
    tag: 'set-span-attribute'
    val: SetSpanAttributeParameters
  } |
  {
    tag: 'change-persistence-level'
    val: ChangePersistenceLevelParameters
  };
}
