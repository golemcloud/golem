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
  AgentId as RawAgentId,
  EnvironmentId as RawEnvironmentId,
} from 'golem:api/oplog@1.5.0';
import type * as OplogTypes from 'golem:api/oplog@1.5.0';

import { Uuid } from '../uuid';
import { ComponentId, AccountId, EnvironmentId } from '../ids';
import type { AgentId } from './hostapi';

// Re-export enriched types for convenience
export { Uuid } from '../uuid';
export { ComponentId, AccountId, EnvironmentId } from '../ids';
export type { AgentId } from './hostapi';

// Re-export types that don't contain UUID-based types
export type Datetime = OplogTypes.Datetime;
export type SchemaValueTree = OplogTypes.SchemaValueTree;
export type ComponentRevision = OplogTypes.ComponentRevision;
export type OplogIndex = OplogTypes.OplogIndex;
export type SnapshotBasedUpdateParameters = OplogTypes.SnapshotBasedUpdateParameters;
export type Attribute = OplogTypes.Attribute;
export type AttributeValue = OplogTypes.AttributeValue;
export type SpanId = OplogTypes.SpanId;
export type TraceId = OplogTypes.TraceId;
export type WrappedFunctionType = OplogTypes.WrappedFunctionType;
export type PluginInstallationDescription = OplogTypes.PluginInstallationDescription;
export type RawLocalAgentConfigEntry = OplogTypes.RawLocalAgentConfigEntry;
export type LocalAgentConfigEntry = OplogTypes.LocalAgentConfigEntry;
export type StartParameters = OplogTypes.StartParameters;
export type EndParameters = OplogTypes.EndParameters;
export type CancelledParameters = OplogTypes.CancelledParameters;
export type SpanData = OplogTypes.SpanData;
export type LocalSpanData = OplogTypes.LocalSpanData;
export type ExternalSpanData = OplogTypes.ExternalSpanData;
export type OplogErrorKind = OplogTypes.OplogErrorKind;
export type ErrorParameters = OplogTypes.ErrorParameters;
export type OplogRegion = OplogTypes.OplogRegion;
export type JumpParameters = OplogTypes.JumpParameters;
export type SetRetryPolicyParameters = OplogTypes.SetRetryPolicyParameters;
export type TypedSchemaValue = OplogTypes.TypedSchemaValue;
export type AgentInitializationParameters = OplogTypes.AgentInitializationParameters;
export type AgentMethodInvocationParameters = OplogTypes.AgentMethodInvocationParameters;
export type LoadSnapshotParameters = OplogTypes.LoadSnapshotParameters;
export type ProcessOplogEntriesParameters = OplogTypes.ProcessOplogEntriesParameters;
export type ManualUpdateParameters = OplogTypes.ManualUpdateParameters;
export type AgentInvocation = OplogTypes.AgentInvocation;
export type AgentInvocationStartedParameters = OplogTypes.AgentInvocationStartedParameters;
export type AgentInvocationOutputParameters = OplogTypes.AgentInvocationOutputParameters;
export type FallibleResultParameters = OplogTypes.FallibleResultParameters;
export type SaveSnapshotResultParameters = OplogTypes.SaveSnapshotResultParameters;
export type AgentInvocationResult = OplogTypes.AgentInvocationResult;
export type AgentInvocationFinishedParameters = OplogTypes.AgentInvocationFinishedParameters;
export type PendingAgentInvocationParameters = OplogTypes.PendingAgentInvocationParameters;
export type UpdateDescription = OplogTypes.UpdateDescription;
export type PendingUpdateParameters = OplogTypes.PendingUpdateParameters;
export type SuccessfulUpdateParameters = OplogTypes.SuccessfulUpdateParameters;
export type FailedUpdateParameters = OplogTypes.FailedUpdateParameters;
export type GrowMemoryParameters = OplogTypes.GrowMemoryParameters;
export type AgentResourceId = OplogTypes.AgentResourceId;
export type CreateResourceParameters = OplogTypes.CreateResourceParameters;
export type DropResourceParameters = OplogTypes.DropResourceParameters;
export type LogLevel = OplogTypes.LogLevel;
export type LogTraceContext = OplogTypes.LogTraceContext;
export type LogParameters = OplogTypes.LogParameters;
export type ActivatePluginParameters = OplogTypes.ActivatePluginParameters;
export type DeactivatePluginParameters = OplogTypes.DeactivatePluginParameters;
export type RevertParameters = OplogTypes.RevertParameters;
export type CancelPendingInvocationParameters = OplogTypes.CancelPendingInvocationParameters;
export type SpanKind = OplogTypes.SpanKind;
export type SpanOutcome = OplogTypes.SpanOutcome;
export type SpanLink = OplogTypes.SpanLink;
export type SpanStarted = OplogTypes.SpanStarted;
export type SpanFinished = OplogTypes.SpanFinished;
export type SpanAttributes = OplogTypes.SpanAttributes;
export type BeginRemoteTransactionParameters = OplogTypes.BeginRemoteTransactionParameters;
export type RemoteTransactionParameters = OplogTypes.RemoteTransactionParameters;
export type SnapshotParameters = OplogTypes.SnapshotParameters;
export type SnapshotConfirmedParameters = OplogTypes.SnapshotConfirmedParameters;
export type Timestamp = OplogTypes.Timestamp;

// Re-export raw/internal types
export type OplogPayload = OplogTypes.OplogPayload;
export type OplogExternalPayload = OplogTypes.OplogExternalPayload;
export type WorkerError = OplogTypes.WorkerError;
export type RawCreateParameters = OplogTypes.RawCreateParameters;
export type RawStartParameters = OplogTypes.RawStartParameters;
export type RawEndParameters = OplogTypes.RawEndParameters;
export type RawCancelledParameters = OplogTypes.RawCancelledParameters;
export type RawCompletionDiscardedParameters = OplogTypes.RawCompletionDiscardedParameters;
export type RawCompletionDeliveredParameters = OplogTypes.RawCompletionDeliveredParameters;
export type RawHostStreamFrameParameters = OplogTypes.RawHostStreamFrameParameters;
export type DurableStreamOutcome = OplogTypes.DurableStreamOutcome;
export type DurableStreamEventSummary = OplogTypes.DurableStreamEventSummary;
export type RawDurableStreamRecordParameters = OplogTypes.RawDurableStreamRecordParameters;
export type RawAgentInvocationStartedParameters = OplogTypes.RawAgentInvocationStartedParameters;
export type RawAgentInvocationFinishedParameters = OplogTypes.RawAgentInvocationFinishedParameters;
export type RawErrorParameters = OplogTypes.RawErrorParameters;
export type RawPendingAgentInvocationParameters = OplogTypes.RawPendingAgentInvocationParameters;
export type RawSnapshotBasedUpdate = OplogTypes.RawSnapshotBasedUpdate;
export type RawUpdateDescription = OplogTypes.RawUpdateDescription;
export type RawPendingUpdateParameters = OplogTypes.RawPendingUpdateParameters;
export type RawSuccessfulUpdateParameters = OplogTypes.RawSuccessfulUpdateParameters;
export type ResourceTypeId = OplogTypes.ResourceTypeId;
export type RawCreateResourceParameters = OplogTypes.RawCreateResourceParameters;
export type RawDropResourceParameters = OplogTypes.RawDropResourceParameters;
export type RawActivatePluginParameters = OplogTypes.RawActivatePluginParameters;
export type RawDeactivatePluginParameters = OplogTypes.RawDeactivatePluginParameters;
export type RawBeginRemoteTransactionParameters = OplogTypes.RawBeginRemoteTransactionParameters;
export type RawSnapshotParameters = OplogTypes.RawSnapshotParameters;
export type RawOplogProcessorCheckpointParameters =
  OplogTypes.RawOplogProcessorCheckpointParameters;
export type OplogEntry = OplogTypes.OplogEntry;

// Enriched types containing UUID-based fields

export type EnvironmentPluginGrantId = {
  uuid: Uuid;
};

export type CreateParameters = {
  timestamp: Datetime;
  agentId: AgentId;
  componentRevision: ComponentRevision;
  env: [string, string][];
  createdBy: AccountId;
  environmentId: EnvironmentId;
  parent?: AgentId;
  componentSize: bigint;
  initialTotalLinearMemorySize: bigint;
  initialActivePlugins: PluginInstallationDescription[];
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
  | { tag: 'start'; val: StartParameters }
  | { tag: 'end'; val: EndParameters }
  | { tag: 'cancelled'; val: CancelledParameters }
  | { tag: 'agent-invocation-started'; val: AgentInvocationStartedParameters }
  | { tag: 'agent-invocation-finished'; val: AgentInvocationFinishedParameters }
  | { tag: 'suspend'; val: Timestamp }
  | { tag: 'error'; val: ErrorParameters }
  | { tag: 'recovery-succeeded'; val: Timestamp }
  | { tag: 'jump'; val: JumpParameters }
  | { tag: 'interrupted'; val: Timestamp }
  | { tag: 'exited'; val: Timestamp }
  | { tag: 'set-retry-policy'; val: SetRetryPolicyParameters }
  | { tag: 'pending-agent-invocation'; val: PendingAgentInvocationParameters }
  | { tag: 'pending-update'; val: PendingUpdateParameters }
  | { tag: 'successful-update'; val: SuccessfulUpdateParameters }
  | { tag: 'failed-update'; val: FailedUpdateParameters }
  | { tag: 'grow-memory'; val: GrowMemoryParameters }
  | { tag: 'create-resource'; val: CreateResourceParameters }
  | { tag: 'drop-resource'; val: DropResourceParameters }
  | { tag: 'log'; val: LogParameters }
  | { tag: 'restart'; val: Timestamp }
  | { tag: 'activate-plugin'; val: ActivatePluginParameters }
  | { tag: 'deactivate-plugin'; val: DeactivatePluginParameters }
  | { tag: 'revert'; val: RevertParameters }
  | { tag: 'cancel-pending-invocation'; val: CancelPendingInvocationParameters }
  | { tag: 'begin-remote-transaction'; val: BeginRemoteTransactionParameters }
  | { tag: 'pre-commit-remote-transaction'; val: RemoteTransactionParameters }
  | { tag: 'pre-rollback-remote-transaction'; val: RemoteTransactionParameters }
  | { tag: 'committed-remote-transaction'; val: RemoteTransactionParameters }
  | { tag: 'rolled-back-remote-transaction'; val: RemoteTransactionParameters }
  | { tag: 'snapshot'; val: SnapshotParameters }
  | { tag: 'snapshot-confirmed'; val: SnapshotConfirmedParameters }
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
      ? raw.map(
          ([idx, entry]) => [idx, wrapPublicOplogEntry(entry)] as [OplogIndex, PublicOplogEntry],
        )
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
