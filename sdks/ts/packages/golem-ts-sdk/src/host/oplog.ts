// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import {
  GetOplog as RawGetOplog,
  SearchOplog as RawSearchOplog,
  enrichOplogEntries as rawEnrichOplogEntries,
  PublicOplogEntry as RawPublicOplogEntry,
  CreateParameters as RawCreateParameters_Public,
  OplogProcessorCheckpointParameters as RawOplogProcessorCheckpointParameters_Public,
  AgentId as RawAgentId,
  EnvironmentId as RawEnvironmentId,
} from 'golem:api/oplog@1.5.0';
import type {
  OplogIndex,
  ComponentRevision,
  Datetime,
  HostCallParameters,
  AgentInvocationStartedParameters,
  AgentInvocationFinishedParameters,
  ErrorParameters,
  JumpParameters,
  ChangeRetryPolicyParameters,
  EndAtomicRegionParameters,
  EndRemoteWriteParameters,
  PendingAgentInvocationParameters,
  PendingUpdateParameters,
  SuccessfulUpdateParameters,
  FailedUpdateParameters,
  GrowMemoryParameters,
  FilesystemStorageUsageUpdateParameters,
  CreateResourceParameters,
  DropResourceParameters,
  LogParameters,
  ActivatePluginParameters,
  DeactivatePluginParameters,
  RevertParameters,
  CancelPendingInvocationParameters,
  StartSpanParameters,
  FinishSpanParameters,
  SetSpanAttributeParameters,
  ChangePersistenceLevelParameters,
  BeginRemoteTransactionParameters,
  RemoteTransactionParameters,
  SnapshotParameters,
  Timestamp,
  PluginInstallationDescription,
  LocalAgentConfigEntry,
} from 'golem:api/oplog@1.5.0';

import { Uuid } from '../uuid';
import { ComponentId, AccountId, EnvironmentId } from '../ids';
import type { AgentId } from './hostapi';

// Re-export enriched types for convenience
export { Uuid } from '../uuid';
export { ComponentId, AccountId, EnvironmentId } from '../ids';
export type { AgentId } from './hostapi';

// Re-export types that don't contain UUID-based types
export type {
  Datetime,
  ValueAndType,
  DataValue,
  DataSchema,
  WitValue,
  ComponentRevision,
  OplogIndex,
  PersistenceLevel,
  RetryPolicy,
  Snapshot,
  Attribute,
  AttributeValue,
  SpanId,
  TraceId,
  WrappedFunctionType,
  PluginInstallationDescription,
  RawLocalAgentConfigEntry,
  LocalAgentConfigEntry,
  HostCallParameters,
  SpanData,
  LocalSpanData,
  ExternalSpanData,
  ErrorParameters,
  OplogRegion,
  JumpParameters,
  ChangeRetryPolicyParameters,
  EndAtomicRegionParameters,
  EndRemoteWriteParameters,
  TypedDataValue,
  AgentInitializationParameters,
  AgentMethodInvocationParameters,
  LoadSnapshotParameters,
  ProcessOplogEntriesParameters,
  ManualUpdateParameters,
  AgentInvocation,
  AgentInvocationStartedParameters,
  AgentInvocationOutputParameters,
  FallibleResultParameters,
  SaveSnapshotResultParameters,
  AgentInvocationResult,
  AgentInvocationFinishedParameters,
  PendingAgentInvocationParameters,
  UpdateDescription,
  PendingUpdateParameters,
  SuccessfulUpdateParameters,
  FailedUpdateParameters,
  GrowMemoryParameters,
  FilesystemStorageUsageUpdateParameters,
  AgentResourceId,
  CreateResourceParameters,
  DropResourceParameters,
  LogLevel,
  LogParameters,
  ActivatePluginParameters,
  DeactivatePluginParameters,
  RevertParameters,
  CancelPendingInvocationParameters,
  StartSpanParameters,
  FinishSpanParameters,
  SetSpanAttributeParameters,
  ChangePersistenceLevelParameters,
  BeginRemoteTransactionParameters,
  RemoteTransactionParameters,
  SnapshotParameters,
  Timestamp,
} from 'golem:api/oplog@1.5.0';

// Re-export raw/internal types
export type {
  OplogPayload,
  OplogExternalPayload,
  WorkerError,
  RawCreateParameters,
  RawHostCallParameters,
  RawAgentInvocationStartedParameters,
  RawAgentInvocationFinishedParameters,
  RawErrorParameters,
  RawPendingAgentInvocationParameters,
  RawSnapshotBasedUpdate,
  RawUpdateDescription,
  RawPendingUpdateParameters,
  RawSuccessfulUpdateParameters,
  ResourceTypeId,
  RawCreateResourceParameters,
  RawDropResourceParameters,
  RawActivatePluginParameters,
  RawDeactivatePluginParameters,
  RawBeginRemoteTransactionParameters,
  RawSnapshotParameters,
  RawOplogProcessorCheckpointParameters,
  OplogEntry,
} from 'golem:api/oplog@1.5.0';

// Enriched types containing UUID-based fields

export type EnvironmentPluginGrantId = {
  uuid: Uuid;
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
  localAgentConfig: LocalAgentConfigEntry[];
};

export type OplogProcessorCheckpointParameters = {
  timestamp: Datetime;
  plugin: PluginInstallationDescription;
  targetAgentId: AgentId;
  confirmedUpTo: OplogIndex;
  sendingUpTo: OplogIndex;
  lastBatchStart: OplogIndex;
};

export type PublicOplogEntry =
  | { tag: 'create'; val: CreateParameters }
  | { tag: 'host-call'; val: HostCallParameters }
  | { tag: 'agent-invocation-started'; val: AgentInvocationStartedParameters }
  | { tag: 'agent-invocation-finished'; val: AgentInvocationFinishedParameters }
  | { tag: 'suspend'; val: Timestamp }
  | { tag: 'error'; val: ErrorParameters }
  | { tag: 'no-op'; val: Timestamp }
  | { tag: 'jump'; val: JumpParameters }
  | { tag: 'interrupted'; val: Timestamp }
  | { tag: 'exited'; val: Timestamp }
  | { tag: 'change-retry-policy'; val: ChangeRetryPolicyParameters }
  | { tag: 'begin-atomic-region'; val: Timestamp }
  | { tag: 'end-atomic-region'; val: EndAtomicRegionParameters }
  | { tag: 'begin-remote-write'; val: Timestamp }
  | { tag: 'end-remote-write'; val: EndRemoteWriteParameters }
  | { tag: 'pending-agent-invocation'; val: PendingAgentInvocationParameters }
  | { tag: 'pending-update'; val: PendingUpdateParameters }
  | { tag: 'successful-update'; val: SuccessfulUpdateParameters }
  | { tag: 'failed-update'; val: FailedUpdateParameters }
  | { tag: 'grow-memory'; val: GrowMemoryParameters }
  | { tag: 'filesystem-storage-usage-update'; val: FilesystemStorageUsageUpdateParameters }
  | { tag: 'create-resource'; val: CreateResourceParameters }
  | { tag: 'drop-resource'; val: DropResourceParameters }
  | { tag: 'log'; val: LogParameters }
  | { tag: 'restart'; val: Timestamp }
  | { tag: 'activate-plugin'; val: ActivatePluginParameters }
  | { tag: 'deactivate-plugin'; val: DeactivatePluginParameters }
  | { tag: 'revert'; val: RevertParameters }
  | { tag: 'cancel-pending-invocation'; val: CancelPendingInvocationParameters }
  | { tag: 'start-span'; val: StartSpanParameters }
  | { tag: 'finish-span'; val: FinishSpanParameters }
  | { tag: 'set-span-attribute'; val: SetSpanAttributeParameters }
  | { tag: 'change-persistence-level'; val: ChangePersistenceLevelParameters }
  | { tag: 'begin-remote-transaction'; val: BeginRemoteTransactionParameters }
  | { tag: 'pre-commit-remote-transaction'; val: RemoteTransactionParameters }
  | { tag: 'pre-rollback-remote-transaction'; val: RemoteTransactionParameters }
  | { tag: 'committed-remote-transaction'; val: RemoteTransactionParameters }
  | { tag: 'rolled-back-remote-transaction'; val: RemoteTransactionParameters }
  | { tag: 'snapshot'; val: SnapshotParameters }
  | { tag: 'oplog-processor-checkpoint'; val: OplogProcessorCheckpointParameters };

// Wrapping helpers

function wrapAgentId(raw: RawAgentId): AgentId {
  return {
    componentId: ComponentId.from(raw.componentId),
    agentId: raw.agentId,
  };
}

function wrapPublicOplogEntry(raw: RawPublicOplogEntry): PublicOplogEntry {
  switch (raw.tag) {
    case 'create':
      return {
        tag: 'create',
        val: {
          ...raw.val,
          agentId: wrapAgentId(raw.val.agentId),
          createdBy: AccountId.from(raw.val.createdBy),
          environmentId: EnvironmentId.from(raw.val.environmentId),
          parent: raw.val.parent ? wrapAgentId(raw.val.parent) : undefined,
        },
      };
    case 'oplog-processor-checkpoint':
      return {
        tag: 'oplog-processor-checkpoint',
        val: {
          ...raw.val,
          targetAgentId: wrapAgentId(raw.val.targetAgentId),
        },
      };
    default:
      return raw as PublicOplogEntry;
  }
}

// Wrapped classes and functions

export class GetOplog {
  private readonly inner: RawGetOplog;

  constructor(agentId: RawAgentId, start: OplogIndex) {
    this.inner = new RawGetOplog(agentId, start);
  }

  getNext(): PublicOplogEntry[] | undefined {
    const raw = this.inner.getNext();
    return raw ? raw.map(wrapPublicOplogEntry) : undefined;
  }
}

export class SearchOplog {
  private readonly inner: RawSearchOplog;

  constructor(agentId: RawAgentId, text: string) {
    this.inner = new RawSearchOplog(agentId, text);
  }

  getNext(): [OplogIndex, PublicOplogEntry][] | undefined {
    const raw = this.inner.getNext();
    return raw
      ? raw.map(([idx, entry]) => [idx, wrapPublicOplogEntry(entry)] as [OplogIndex, PublicOplogEntry])
      : undefined;
  }
}

export function enrichOplogEntries(
  environmentId: RawEnvironmentId,
  agentId: RawAgentId,
  entries: [OplogIndex, import('golem:api/oplog@1.5.0').OplogEntry][],
  componentRevision: ComponentRevision,
): PublicOplogEntry[] {
  return rawEnrichOplogEntries(environmentId, agentId, entries, componentRevision).map(
    wrapPublicOplogEntry,
  );
}
