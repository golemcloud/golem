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

//! The host-call decision table of a never-live agent.
//!
//! A never-live agent replays its history for a pending automatic update and stops. A durable
//! call that can only return its recorded result is always admitted, so recorded replay does not
//! change. A durable call that executes now (a snapshot hook runs with durability suppressed, and
//! a bootstrap cursor with a live tail executes the initializer's next calls) is admitted only
//! when its host function is allowed here. The allowed functions are local: clocks, random,
//! `cli::environment`, the private filesystem, `io::poll`, invocation-context spans,
//! `wasi:config/store` and the agent's own metadata. Every other function is refused, and so is
//! every `Custom` function, because the guest supplies its name and function type.
//!
//! The match is exhaustive, so a new host function does not compile until it has a decision.

use golem_common::model::oplog::host_functions::HostFunctionName;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::{Arc, OnceLock};

/// The taint of one generation: set once, with its first source, when the guest of the
/// generation observed a result that the oplog does not record. In a tainted generation, a
/// divergence or a failure of the target code proves no incompatibility, so the outcome table
/// classes it as an operational failure.
#[derive(Clone, Debug, Default)]
pub(crate) struct GenerationTaint(Arc<OnceLock<String>>);

impl GenerationTaint {
    /// Taints the generation with `source`, unless it is already tainted.
    pub(crate) fn set(&self, source: impl FnOnce() -> String) {
        let _ = self.0.get_or_init(source);
    }

    pub(crate) fn is_tainted(&self) -> bool {
        self.0.get().is_some()
    }
}

/// The refusal of a peer effect at `boundary` for a never-live owner.
pub(crate) fn refusal(boundary: &str) -> WorkerExecutorError {
    tracing::warn!(boundary, "a never-live owner refused a peer effect");
    WorkerExecutorError::NeverLiveEffectRefused {
        boundary: boundary.to_string(),
    }
}

/// What a never-live agent may do with a host call that executes now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Execution {
    /// The call is refused: it would reach a peer, or its effect is not local.
    Refused,
    /// The call is local, and its result comes from pinned or deterministic state.
    Allowed,
    /// The call is local, but the guest observes a result that the oplog does not record: a
    /// clock, random or environment read, or local filesystem data. It taints the generation.
    AllowedUnrecorded,
}

/// The decision of the host-call decision table for `host_function` when it executes now.
pub(crate) fn execution(host_function: &HostFunctionName) -> Execution {
    match host_function {
        HostFunctionName::RandomInsecureSeedInsecureSeed
        | HostFunctionName::RandomInsecureGetInsecureRandomBytes
        | HostFunctionName::RandomInsecureGetInsecureRandomU64
        | HostFunctionName::RandomGetRandomBytes
        | HostFunctionName::RandomGetRandomU64
        | HostFunctionName::WallClockNow
        | HostFunctionName::WallClockResolution
        | HostFunctionName::MonotonicClockNow
        | HostFunctionName::MonotonicClockResolution
        | HostFunctionName::MonotonicClockSubscribeDuration
        | HostFunctionName::P3MonotonicClockWaitFor
        | HostFunctionName::FilesystemTypesDescriptorStat
        | HostFunctionName::FilesystemTypesDescriptorStatAt
        | HostFunctionName::P3SystemClockNow
        | HostFunctionName::P3SystemClockGetResolution
        | HostFunctionName::P3MonotonicClockNow
        | HostFunctionName::P3MonotonicClockGetResolution
        | HostFunctionName::P3MonotonicClockWaitUntil
        | HostFunctionName::P3RandomRandomGetRandomBytes
        | HostFunctionName::P3RandomRandomGetRandomU64
        | HostFunctionName::P3RandomInsecureGetInsecureRandomBytes
        | HostFunctionName::P3RandomInsecureGetInsecureRandomU64
        | HostFunctionName::P3RandomInsecureSeedGetInsecureSeed
        | HostFunctionName::P3FilesystemTypesDescriptorStat
        | HostFunctionName::P3FilesystemTypesDescriptorStatAt
        | HostFunctionName::FilesystemInputStreamRead
        | HostFunctionName::FilesystemInputStreamSkip
        | HostFunctionName::FilesystemOutputStreamCheckWrite
        | HostFunctionName::P3FilesystemTypesDescriptorWriteViaStream
        | HostFunctionName::P3FilesystemTypesDescriptorAppendViaStream
        | HostFunctionName::WasiCliEnvironmentGetEnvironment
        | HostFunctionName::FilesystemPermissionCheck => Execution::AllowedUnrecorded,
        HostFunctionName::IoPollReady
        | HostFunctionName::IoPollPoll
        | HostFunctionName::GolemApiGetSelfMetadata
        | HostFunctionName::GolemContextStartSpan
        | HostFunctionName::GolemContextSpanFinish
        | HostFunctionName::GolemContextSpanSetAttributes
        | HostFunctionName::WasiConfigGet
        | HostFunctionName::WasiConfigGetAll => Execution::Allowed,
        HostFunctionName::RdbmsMysqlDbConnectionExecute
        | HostFunctionName::RdbmsMysqlDbConnectionQuery
        | HostFunctionName::RdbmsMysqlDbConnectionQueryStream
        | HostFunctionName::RdbmsMysqlDbResultStreamGetColumns
        | HostFunctionName::RdbmsMysqlDbResultStreamGetNext
        | HostFunctionName::RdbmsMysqlDbTransactionQuery
        | HostFunctionName::RdbmsMysqlDbTransactionExecute
        | HostFunctionName::RdbmsMysqlDbTransactionQueryStream
        | HostFunctionName::RdbmsPostgresDbConnectionExecute
        | HostFunctionName::RdbmsPostgresDbConnectionQuery
        | HostFunctionName::RdbmsPostgresDbConnectionQueryStream
        | HostFunctionName::RdbmsPostgresDbResultStreamGetColumns
        | HostFunctionName::RdbmsPostgresDbResultStreamGetNext
        | HostFunctionName::RdbmsPostgresDbTransactionQuery
        | HostFunctionName::RdbmsPostgresDbTransactionExecute
        | HostFunctionName::RdbmsPostgresDbTransactionQueryStream
        | HostFunctionName::RdbmsIgnite2DbConnectionExecute
        | HostFunctionName::RdbmsIgnite2DbConnectionQuery
        | HostFunctionName::RdbmsIgnite2DbConnectionQueryStream
        | HostFunctionName::RdbmsIgnite2DbResultStreamGetColumns
        | HostFunctionName::RdbmsIgnite2DbResultStreamGetNext
        | HostFunctionName::RdbmsIgnite2DbTransactionQuery
        | HostFunctionName::RdbmsIgnite2DbTransactionExecute
        | HostFunctionName::RdbmsIgnite2DbTransactionQueryStream
        | HostFunctionName::KeyvalueEventualGet
        | HostFunctionName::KeyvalueEventualSet
        | HostFunctionName::KeyvalueEventualDelete
        | HostFunctionName::KeyvalueEventualExists
        | HostFunctionName::KeyvalueEventualBatchGetMany
        | HostFunctionName::KeyvalueEventualBatchGetKeys
        | HostFunctionName::KeyvalueEventualBatchSetMany
        | HostFunctionName::KeyvalueEventualBatchDeleteMany
        | HostFunctionName::GolemRpcFutureInvokeResultGet
        | HostFunctionName::GolemRpcWasmRpcInvokeAndAwaitResult
        | HostFunctionName::GolemRpcWasmRpcInvoke
        | HostFunctionName::GolemRpcWasmRpcScheduleInvocation
        | HostFunctionName::GolemRpcCancellationTokenCancel
        | HostFunctionName::GolemRpcFutureInvokeResultCancel
        | HostFunctionName::BlobstoreBlobstoreCreateContainer
        | HostFunctionName::BlobstoreBlobstoreGetContainer
        | HostFunctionName::BlobstoreBlobstoreDeleteContainer
        | HostFunctionName::BlobstoreBlobstoreContainerExists
        | HostFunctionName::BlobstoreBlobstoreCopyObject
        | HostFunctionName::BlobstoreBlobstoreMoveObject
        | HostFunctionName::BlobstoreContainerGetData
        | HostFunctionName::BlobstoreContainerWriteData
        | HostFunctionName::BlobstoreContainerListObject
        | HostFunctionName::BlobstoreContainerDeleteObject
        | HostFunctionName::BlobstoreContainerDeleteObjects
        | HostFunctionName::BlobstoreContainerHasObject
        | HostFunctionName::BlobstoreContainerObjectInfo
        | HostFunctionName::BlobstoreContainerClear
        | HostFunctionName::SocketsIpNameLookupResolveAddresses
        | HostFunctionName::P3SocketsIpNameLookupResolveAddresses
        | HostFunctionName::GolemNetworkDnsResolve
        | HostFunctionName::GolemAgentGetAllAgentTypes
        | HostFunctionName::GolemAgentGetAgentType
        | HostFunctionName::GolemAgentCreateWebhook
        | HostFunctionName::GolemAgentGetConfigValue
        | HostFunctionName::GolemAgentDurableStreamReaderNew
        | HostFunctionName::GolemAgentDurableStreamWriterNew
        | HostFunctionName::GolemAgentDurableStreamReaderRead
        | HostFunctionName::GolemAgentDurableStreamWriterAppend
        | HostFunctionName::GolemApiCreatePromise
        | HostFunctionName::GolemApiCompletePromise
        | HostFunctionName::GolemApiGenerateIdempotencyKey
        | HostFunctionName::GolemApiUpdateWorker
        | HostFunctionName::GolemApiGetAgentMetadata
        | HostFunctionName::GolemApiGetPromiseResult
        | HostFunctionName::GolemApiForkWorker
        | HostFunctionName::GolemApiRevertWorker
        | HostFunctionName::GolemApiResolveComponentId
        | HostFunctionName::GolemApiResolveAgentIdStrict
        | HostFunctionName::GolemApiFork
        | HostFunctionName::WebsocketClientConnect
        | HostFunctionName::WebsocketClientSend
        | HostFunctionName::WebsocketClientReceive
        | HostFunctionName::WebsocketClientReceiveWithTimeout
        | HostFunctionName::WebsocketClientClose
        | HostFunctionName::GolemQuotaTokenNew
        | HostFunctionName::GolemQuotaTokenReserve
        | HostFunctionName::GolemQuotaReservationCommit
        | HostFunctionName::GolemApiRetryGetRetryPolicies
        | HostFunctionName::GolemApiRetryGetRetryPolicyByName
        | HostFunctionName::GolemApiRetryResolveRetryPolicy
        | HostFunctionName::GolemApiRetryNextRetryDelay
        | HostFunctionName::GolemRpcWasmRpcNew
        | HostFunctionName::GolemRpcWasmRpcBindSelf
        | HostFunctionName::GolemRpcWasmRpcAsyncInvokeRejection
        | HostFunctionName::P3KeyvalueCacheGet
        | HostFunctionName::P3KeyvalueCacheExists
        | HostFunctionName::P3KeyvalueCacheSet
        | HostFunctionName::P3KeyvalueCacheGetOrSet
        | HostFunctionName::P3KeyvalueCacheDelete
        | HostFunctionName::P3KeyvalueCacheVacancyFill
        | HostFunctionName::P3KeyvalueTypesIncomingValueConsumeAsync
        | HostFunctionName::P3BlobstoreTypesIncomingValueConsumeAsync
        | HostFunctionName::P3SocketsTypesTcpSocketSend
        | HostFunctionName::P3SocketsTypesTcpSocketReceive
        | HostFunctionName::P3SocketsTypesTcpSocketReceiveChunk
        | HostFunctionName::P3SocketsTypesTcpSocketSendAcquire
        | HostFunctionName::P3SocketsTypesTcpSocketReceiveAcquire
        | HostFunctionName::P3SocketsTypesTcpSocketSendChunk
        | HostFunctionName::P3SocketsTypesUdpSocketSend
        | HostFunctionName::P3SocketsTypesUdpSocketReceive
        | HostFunctionName::P3HttpClientSend
        | HostFunctionName::P3HttpClientConsumeBody
        | HostFunctionName::P3HttpClientConsumeBodyChunk
        | HostFunctionName::P3HttpClientRequestBodyTransmission
        | HostFunctionName::GolemSecretsReveal
        | HostFunctionName::GolemApiDeriveCard
        | HostFunctionName::GolemApiInstallCard
        | HostFunctionName::GolemPermissionsDerivePersist
        | HostFunctionName::GolemPermissionsRevokePersist
        | HostFunctionName::GolemPermissionsCardLiveness
        | HostFunctionName::GolemPermissionsInstallChildPersist
        | HostFunctionName::GolemPermissionsInstallTransfer
        | HostFunctionName::GolemPermissionsInspectDecision
        | HostFunctionName::GolemPermissionsDeriveDecision
        | HostFunctionName::GolemPermissionsDeriveFromWalletDecision
        | HostFunctionName::GolemPermissionsDeriveScopeDecision
        | HostFunctionName::GolemPermissionsInstallDecision
        | HostFunctionName::GolemToolGetAllTools
        | HostFunctionName::GolemToolGetTool
        | HostFunctionName::RdbmsMysqlDbConnectionBeginTransaction
        | HostFunctionName::RdbmsPostgresDbConnectionBeginTransaction
        | HostFunctionName::RdbmsIgnite2DbConnectionBeginTransaction
        | HostFunctionName::GolemApiGetOplogNext
        | HostFunctionName::GolemApiSearchOplogNext
        | HostFunctionName::GolemApiEnrichOplogEntries
        | HostFunctionName::P3SocketsTypesTcpSocketConnect
        | HostFunctionName::P3SocketsTypesTcpSocketBind
        | HostFunctionName::P3SocketsTypesUdpSocketConnect
        | HostFunctionName::P3SocketsTypesUdpSocketBind
        | HostFunctionName::P3SocketsTypesUdpSocketDisconnect
        | HostFunctionName::GolemToolRpcInvokeAndAwait
        | HostFunctionName::GolemToolRpcInvoke
        | HostFunctionName::GolemToolRpcAsyncInvokeAndAwait
        | HostFunctionName::GolemApiGetAgents
        | HostFunctionName::GolemRpcWasmRpcPrepareInvocation
        | HostFunctionName::GolemToolResponseSecretHoldAdmission
        | HostFunctionName::GolemToolAwaitInvokeResults
        | HostFunctionName::GolemEntityInvoke
        | HostFunctionName::GolemToolInvocationRejected
        | HostFunctionName::McpToolCall
        | HostFunctionName::McpToolPresence
        | HostFunctionName::GolemAgentGetAgentTypeByAgentId
        | HostFunctionName::GolemToolCreateStdin
        | HostFunctionName::GolemToolCreateOutput
        | HostFunctionName::GolemToolStdinWrite
        | HostFunctionName::GolemToolStdinFinish
        | HostFunctionName::GolemToolStdinFail
        | HostFunctionName::GolemToolStdinWait
        | HostFunctionName::GolemToolOutputWrite
        | HostFunctionName::GolemToolOutputFinish
        | HostFunctionName::GolemToolOutputFail
        | HostFunctionName::SocketsTcpConnectPermissionCheck
        | HostFunctionName::SocketsUdpPermissionCheck
        | HostFunctionName::SocketsUdpStartBind
        | HostFunctionName::SocketsUdpStream
        | HostFunctionName::SocketsUdpReceive
        | HostFunctionName::SocketsUdpCheckSend
        | HostFunctionName::SocketsUdpSend
        | HostFunctionName::GolemPermissionsSelfWallet
        | HostFunctionName::GolemPermissionsSelfVersion
        | HostFunctionName::Custom(_) => Execution::Refused,
    }
}

#[cfg(test)]
mod tests;
