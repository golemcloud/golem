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

//! Host payloads: `Start.request`, `End.response` and `Cancelled.partial`.
//!
//! Trust comes from the host function and the direction, never from a type ID in the value. A
//! payload is trusted only when its host function is known, the (function, direction) pair is
//! not one whose public value can be bare customer data, and the value's graph equals the graph
//! of the pair's Rust type. Every other payload is customer data as a whole.

use super::value::{Region, Walker, normalized, number, record, resolve, text, unsigned};
use crate::oplog_export::model::{ReportField, ReportNode, TupleNode};
use golem_common::model::oplog::host_functions::{self, HostFunctionName};
use golem_common::model::oplog::{
    HostPayloadPair, HostRequestGolemAgentGetConfigValue, HostRequestGolemApiForkAgent,
    HostRequestGolemRpcInvoke, HostRequestGolemRpcScheduledInvocation, HostRequestRandomBytes,
    HostResponseGolemAgentAgentType, HostResponseGolemAgentAgentTypes,
    HostResponseGolemRpcInvokeAndAwait,
};
use golem_common::schema::{
    IntoSchema, SchemaGraph, SchemaType, SchemaValue, TypeId, TypedSchemaValue,
    try_into_schema_graph,
};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    Request,
    Response,
}

/// The (function, direction) pairs whose public value is the bare customer value: the
/// executor unwraps the payload before it writes the public entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bare {
    Neither,
    Request,
    Response,
    Both,
}

/// The Rust graphs of one host function. `None` for a direction that is never trusted.
struct PairGraphs {
    request: Option<SchemaGraph>,
    response: Option<SchemaGraph>,
}

fn pair<P: HostPayloadPair>(bare: Bare) -> Option<PairGraphs>
where
    P::Req: IntoSchema,
    P::Resp: IntoSchema,
{
    let request = matches!(bare, Bare::Neither | Bare::Response)
        .then(|| try_into_schema_graph::<P::Req>().ok())
        .flatten();
    let response = matches!(bare, Bare::Neither | Bare::Request)
        .then(|| try_into_schema_graph::<P::Resp>().ok())
        .flatten();
    Some(PairGraphs { request, response })
}

/// One arm per host function, with no `_` arm, so a new host function compiles only with an
/// explicit entry.
fn pair_graphs(name: &HostFunctionName) -> Option<PairGraphs> {
    match name {
        HostFunctionName::Custom(_) => None,
        HostFunctionName::RdbmsMysqlDbConnectionExecute => {
            pair::<host_functions::RdbmsMysqlDbConnectionExecute>(Bare::Neither)
        }
        HostFunctionName::RdbmsMysqlDbConnectionQuery => {
            pair::<host_functions::RdbmsMysqlDbConnectionQuery>(Bare::Neither)
        }
        HostFunctionName::RdbmsMysqlDbConnectionQueryStream => {
            pair::<host_functions::RdbmsMysqlDbConnectionQueryStream>(Bare::Neither)
        }
        HostFunctionName::RdbmsMysqlDbResultStreamGetColumns => {
            pair::<host_functions::RdbmsMysqlDbResultStreamGetColumns>(Bare::Neither)
        }
        HostFunctionName::RdbmsMysqlDbResultStreamGetNext => {
            pair::<host_functions::RdbmsMysqlDbResultStreamGetNext>(Bare::Neither)
        }
        HostFunctionName::RdbmsMysqlDbTransactionQuery => {
            pair::<host_functions::RdbmsMysqlDbTransactionQuery>(Bare::Neither)
        }
        HostFunctionName::RdbmsMysqlDbTransactionExecute => {
            pair::<host_functions::RdbmsMysqlDbTransactionExecute>(Bare::Neither)
        }
        HostFunctionName::RdbmsMysqlDbTransactionQueryStream => {
            pair::<host_functions::RdbmsMysqlDbTransactionQueryStream>(Bare::Neither)
        }
        HostFunctionName::RdbmsPostgresDbConnectionExecute => {
            pair::<host_functions::RdbmsPostgresDbConnectionExecute>(Bare::Neither)
        }
        HostFunctionName::RdbmsPostgresDbConnectionQuery => {
            pair::<host_functions::RdbmsPostgresDbConnectionQuery>(Bare::Neither)
        }
        HostFunctionName::RdbmsPostgresDbConnectionQueryStream => {
            pair::<host_functions::RdbmsPostgresDbConnectionQueryStream>(Bare::Neither)
        }
        HostFunctionName::RdbmsPostgresDbResultStreamGetColumns => {
            pair::<host_functions::RdbmsPostgresDbResultStreamGetColumns>(Bare::Neither)
        }
        HostFunctionName::RdbmsPostgresDbResultStreamGetNext => {
            pair::<host_functions::RdbmsPostgresDbResultStreamGetNext>(Bare::Neither)
        }
        HostFunctionName::RdbmsPostgresDbTransactionQuery => {
            pair::<host_functions::RdbmsPostgresDbTransactionQuery>(Bare::Neither)
        }
        HostFunctionName::RdbmsPostgresDbTransactionExecute => {
            pair::<host_functions::RdbmsPostgresDbTransactionExecute>(Bare::Neither)
        }
        HostFunctionName::RdbmsPostgresDbTransactionQueryStream => {
            pair::<host_functions::RdbmsPostgresDbTransactionQueryStream>(Bare::Neither)
        }
        HostFunctionName::RdbmsIgnite2DbConnectionExecute => {
            pair::<host_functions::RdbmsIgnite2DbConnectionExecute>(Bare::Neither)
        }
        HostFunctionName::RdbmsIgnite2DbConnectionQuery => {
            pair::<host_functions::RdbmsIgnite2DbConnectionQuery>(Bare::Neither)
        }
        HostFunctionName::RdbmsIgnite2DbConnectionQueryStream => {
            pair::<host_functions::RdbmsIgnite2DbConnectionQueryStream>(Bare::Neither)
        }
        HostFunctionName::RdbmsIgnite2DbResultStreamGetColumns => {
            pair::<host_functions::RdbmsIgnite2DbResultStreamGetColumns>(Bare::Neither)
        }
        HostFunctionName::RdbmsIgnite2DbResultStreamGetNext => {
            pair::<host_functions::RdbmsIgnite2DbResultStreamGetNext>(Bare::Neither)
        }
        HostFunctionName::RdbmsIgnite2DbTransactionQuery => {
            pair::<host_functions::RdbmsIgnite2DbTransactionQuery>(Bare::Neither)
        }
        HostFunctionName::RdbmsIgnite2DbTransactionExecute => {
            pair::<host_functions::RdbmsIgnite2DbTransactionExecute>(Bare::Neither)
        }
        HostFunctionName::RdbmsIgnite2DbTransactionQueryStream => {
            pair::<host_functions::RdbmsIgnite2DbTransactionQueryStream>(Bare::Neither)
        }
        HostFunctionName::KeyvalueEventualGet => {
            pair::<host_functions::KeyvalueEventualGet>(Bare::Neither)
        }
        HostFunctionName::KeyvalueEventualSet => {
            pair::<host_functions::KeyvalueEventualSet>(Bare::Neither)
        }
        HostFunctionName::KeyvalueEventualDelete => {
            pair::<host_functions::KeyvalueEventualDelete>(Bare::Neither)
        }
        HostFunctionName::KeyvalueEventualExists => {
            pair::<host_functions::KeyvalueEventualExists>(Bare::Neither)
        }
        HostFunctionName::KeyvalueEventualBatchGetMany => {
            pair::<host_functions::KeyvalueEventualBatchGetMany>(Bare::Neither)
        }
        HostFunctionName::KeyvalueEventualBatchGetKeys => {
            pair::<host_functions::KeyvalueEventualBatchGetKeys>(Bare::Neither)
        }
        HostFunctionName::KeyvalueEventualBatchSetMany => {
            pair::<host_functions::KeyvalueEventualBatchSetMany>(Bare::Neither)
        }
        HostFunctionName::KeyvalueEventualBatchDeleteMany => {
            pair::<host_functions::KeyvalueEventualBatchDeleteMany>(Bare::Neither)
        }
        HostFunctionName::RandomInsecureSeedInsecureSeed => {
            pair::<host_functions::RandomInsecureSeedInsecureSeed>(Bare::Neither)
        }
        HostFunctionName::RandomInsecureGetInsecureRandomBytes => {
            pair::<host_functions::RandomInsecureGetInsecureRandomBytes>(Bare::Neither)
        }
        HostFunctionName::RandomInsecureGetInsecureRandomU64 => {
            pair::<host_functions::RandomInsecureGetInsecureRandomU64>(Bare::Neither)
        }
        HostFunctionName::RandomGetRandomBytes => {
            pair::<host_functions::RandomGetRandomBytes>(Bare::Neither)
        }
        HostFunctionName::RandomGetRandomU64 => {
            pair::<host_functions::RandomGetRandomU64>(Bare::Neither)
        }
        HostFunctionName::GolemRpcFutureInvokeResultGet => {
            pair::<host_functions::GolemRpcFutureInvokeResultGet>(Bare::Neither)
        }
        HostFunctionName::GolemRpcWasmRpcInvokeAndAwaitResult => {
            pair::<host_functions::GolemRpcWasmRpcInvokeAndAwaitResult>(Bare::Neither)
        }
        HostFunctionName::GolemRpcWasmRpcInvoke => {
            pair::<host_functions::GolemRpcWasmRpcInvoke>(Bare::Neither)
        }
        HostFunctionName::GolemRpcWasmRpcScheduleInvocation => {
            pair::<host_functions::GolemRpcWasmRpcScheduleInvocation>(Bare::Neither)
        }
        HostFunctionName::GolemRpcCancellationTokenCancel => {
            pair::<host_functions::GolemRpcCancellationTokenCancel>(Bare::Neither)
        }
        HostFunctionName::GolemRpcFutureInvokeResultCancel => {
            pair::<host_functions::GolemRpcFutureInvokeResultCancel>(Bare::Neither)
        }
        HostFunctionName::IoPollReady => pair::<host_functions::IoPollReady>(Bare::Neither),
        HostFunctionName::IoPollPoll => pair::<host_functions::IoPollPoll>(Bare::Neither),
        HostFunctionName::WallClockNow => pair::<host_functions::WallClockNow>(Bare::Neither),
        HostFunctionName::WallClockResolution => {
            pair::<host_functions::WallClockResolution>(Bare::Neither)
        }
        HostFunctionName::MonotonicClockNow => {
            pair::<host_functions::MonotonicClockNow>(Bare::Neither)
        }
        HostFunctionName::MonotonicClockResolution => {
            pair::<host_functions::MonotonicClockResolution>(Bare::Neither)
        }
        HostFunctionName::MonotonicClockSubscribeDuration => {
            pair::<host_functions::MonotonicClockSubscribeDuration>(Bare::Neither)
        }
        HostFunctionName::P3MonotonicClockWaitFor => {
            pair::<host_functions::P3MonotonicClockWaitFor>(Bare::Neither)
        }
        HostFunctionName::BlobstoreBlobstoreCreateContainer => {
            pair::<host_functions::BlobstoreBlobstoreCreateContainer>(Bare::Neither)
        }
        HostFunctionName::BlobstoreBlobstoreGetContainer => {
            pair::<host_functions::BlobstoreBlobstoreGetContainer>(Bare::Neither)
        }
        HostFunctionName::BlobstoreBlobstoreDeleteContainer => {
            pair::<host_functions::BlobstoreBlobstoreDeleteContainer>(Bare::Neither)
        }
        HostFunctionName::BlobstoreBlobstoreContainerExists => {
            pair::<host_functions::BlobstoreBlobstoreContainerExists>(Bare::Neither)
        }
        HostFunctionName::BlobstoreBlobstoreCopyObject => {
            pair::<host_functions::BlobstoreBlobstoreCopyObject>(Bare::Neither)
        }
        HostFunctionName::BlobstoreBlobstoreMoveObject => {
            pair::<host_functions::BlobstoreBlobstoreMoveObject>(Bare::Neither)
        }
        HostFunctionName::BlobstoreContainerGetData => {
            pair::<host_functions::BlobstoreContainerGetData>(Bare::Neither)
        }
        HostFunctionName::BlobstoreContainerWriteData => {
            pair::<host_functions::BlobstoreContainerWriteData>(Bare::Neither)
        }
        HostFunctionName::BlobstoreContainerListObject => {
            pair::<host_functions::BlobstoreContainerListObject>(Bare::Neither)
        }
        HostFunctionName::BlobstoreContainerDeleteObject => {
            pair::<host_functions::BlobstoreContainerDeleteObject>(Bare::Neither)
        }
        HostFunctionName::BlobstoreContainerDeleteObjects => {
            pair::<host_functions::BlobstoreContainerDeleteObjects>(Bare::Neither)
        }
        HostFunctionName::BlobstoreContainerHasObject => {
            pair::<host_functions::BlobstoreContainerHasObject>(Bare::Neither)
        }
        HostFunctionName::BlobstoreContainerObjectInfo => {
            pair::<host_functions::BlobstoreContainerObjectInfo>(Bare::Neither)
        }
        HostFunctionName::BlobstoreContainerClear => {
            pair::<host_functions::BlobstoreContainerClear>(Bare::Neither)
        }
        HostFunctionName::FilesystemTypesDescriptorStat => {
            pair::<host_functions::FilesystemTypesDescriptorStat>(Bare::Neither)
        }
        HostFunctionName::FilesystemTypesDescriptorStatAt => {
            pair::<host_functions::FilesystemTypesDescriptorStatAt>(Bare::Neither)
        }
        HostFunctionName::SocketsIpNameLookupResolveAddresses => {
            pair::<host_functions::SocketsIpNameLookupResolveAddresses>(Bare::Neither)
        }
        HostFunctionName::P3SocketsIpNameLookupResolveAddresses => {
            pair::<host_functions::P3SocketsIpNameLookupResolveAddresses>(Bare::Neither)
        }
        HostFunctionName::GolemAgentGetAllAgentTypes => {
            pair::<host_functions::GolemAgentGetAllAgentTypes>(Bare::Neither)
        }
        HostFunctionName::GolemAgentGetAgentType => {
            pair::<host_functions::GolemAgentGetAgentType>(Bare::Neither)
        }
        HostFunctionName::GolemAgentCreateWebhook => {
            pair::<host_functions::GolemAgentCreateWebhook>(Bare::Neither)
        }
        HostFunctionName::GolemAgentGetConfigValue => {
            pair::<host_functions::GolemAgentGetConfigValue>(Bare::Neither)
        }
        HostFunctionName::GolemAgentDurableStreamReaderNew => {
            pair::<host_functions::GolemAgentDurableStreamReaderNew>(Bare::Neither)
        }
        HostFunctionName::GolemAgentDurableStreamWriterNew => {
            pair::<host_functions::GolemAgentDurableStreamWriterNew>(Bare::Neither)
        }
        HostFunctionName::GolemAgentDurableStreamReaderRead => {
            pair::<host_functions::GolemAgentDurableStreamReaderRead>(Bare::Neither)
        }
        HostFunctionName::GolemAgentDurableStreamWriterAppend => {
            pair::<host_functions::GolemAgentDurableStreamWriterAppend>(Bare::Neither)
        }
        HostFunctionName::GolemApiCreatePromise => {
            pair::<host_functions::GolemApiCreatePromise>(Bare::Neither)
        }
        HostFunctionName::GolemApiCompletePromise => {
            pair::<host_functions::GolemApiCompletePromise>(Bare::Neither)
        }
        HostFunctionName::GolemApiGenerateIdempotencyKey => {
            pair::<host_functions::GolemApiGenerateIdempotencyKey>(Bare::Neither)
        }
        HostFunctionName::GolemApiUpdateWorker => {
            pair::<host_functions::GolemApiUpdateWorker>(Bare::Neither)
        }
        HostFunctionName::GolemApiGetSelfMetadata => {
            pair::<host_functions::GolemApiGetSelfMetadata>(Bare::Neither)
        }
        HostFunctionName::GolemApiGetAgentMetadata => {
            pair::<host_functions::GolemApiGetAgentMetadata>(Bare::Neither)
        }
        HostFunctionName::GolemApiGetPromiseResult => {
            pair::<host_functions::GolemApiGetPromiseResult>(Bare::Neither)
        }
        HostFunctionName::GolemApiForkWorker => {
            pair::<host_functions::GolemApiForkWorker>(Bare::Neither)
        }
        HostFunctionName::GolemApiRevertWorker => {
            pair::<host_functions::GolemApiRevertWorker>(Bare::Neither)
        }
        HostFunctionName::GolemApiResolveComponentId => {
            pair::<host_functions::GolemApiResolveComponentId>(Bare::Neither)
        }
        HostFunctionName::GolemApiResolveAgentIdStrict => {
            pair::<host_functions::GolemApiResolveAgentIdStrict>(Bare::Neither)
        }
        HostFunctionName::GolemApiFork => pair::<host_functions::GolemApiFork>(Bare::Neither),
        HostFunctionName::WebsocketClientConnect => {
            pair::<host_functions::WebsocketClientConnect>(Bare::Neither)
        }
        HostFunctionName::WebsocketClientSend => {
            pair::<host_functions::WebsocketClientSend>(Bare::Neither)
        }
        HostFunctionName::WebsocketClientReceive => {
            pair::<host_functions::WebsocketClientReceive>(Bare::Neither)
        }
        HostFunctionName::WebsocketClientReceiveWithTimeout => {
            pair::<host_functions::WebsocketClientReceiveWithTimeout>(Bare::Neither)
        }
        HostFunctionName::WebsocketClientClose => {
            pair::<host_functions::WebsocketClientClose>(Bare::Neither)
        }
        HostFunctionName::GolemQuotaTokenNew => {
            pair::<host_functions::GolemQuotaTokenNew>(Bare::Neither)
        }
        HostFunctionName::GolemQuotaTokenReserve => {
            pair::<host_functions::GolemQuotaTokenReserve>(Bare::Neither)
        }
        HostFunctionName::GolemQuotaReservationCommit => {
            pair::<host_functions::GolemQuotaReservationCommit>(Bare::Neither)
        }
        HostFunctionName::GolemApiRetryGetRetryPolicies => {
            pair::<host_functions::GolemApiRetryGetRetryPolicies>(Bare::Neither)
        }
        HostFunctionName::GolemApiRetryGetRetryPolicyByName => {
            pair::<host_functions::GolemApiRetryGetRetryPolicyByName>(Bare::Neither)
        }
        HostFunctionName::GolemApiRetryResolveRetryPolicy => {
            pair::<host_functions::GolemApiRetryResolveRetryPolicy>(Bare::Neither)
        }
        HostFunctionName::GolemApiRetryNextRetryDelay => {
            pair::<host_functions::GolemApiRetryNextRetryDelay>(Bare::Neither)
        }
        HostFunctionName::GolemRpcWasmRpcNew => {
            pair::<host_functions::GolemRpcWasmRpcNew>(Bare::Neither)
        }
        HostFunctionName::GolemRpcWasmRpcAsyncInvokeRejection => {
            pair::<host_functions::GolemRpcWasmRpcAsyncInvokeRejection>(Bare::Neither)
        }
        HostFunctionName::GolemContextStartSpan => {
            pair::<host_functions::GolemContextStartSpan>(Bare::Neither)
        }
        HostFunctionName::GolemContextSpanFinish => {
            pair::<host_functions::GolemContextSpanFinish>(Bare::Neither)
        }
        HostFunctionName::GolemContextSpanSetAttributes => {
            pair::<host_functions::GolemContextSpanSetAttributes>(Bare::Neither)
        }
        HostFunctionName::P3KeyvalueCacheGet => {
            pair::<host_functions::P3KeyvalueCacheGet>(Bare::Neither)
        }
        HostFunctionName::P3KeyvalueCacheExists => {
            pair::<host_functions::P3KeyvalueCacheExists>(Bare::Neither)
        }
        HostFunctionName::P3KeyvalueCacheSet => {
            pair::<host_functions::P3KeyvalueCacheSet>(Bare::Neither)
        }
        HostFunctionName::P3KeyvalueCacheGetOrSet => {
            pair::<host_functions::P3KeyvalueCacheGetOrSet>(Bare::Neither)
        }
        HostFunctionName::P3KeyvalueCacheDelete => {
            pair::<host_functions::P3KeyvalueCacheDelete>(Bare::Neither)
        }
        HostFunctionName::P3KeyvalueCacheVacancyFill => {
            pair::<host_functions::P3KeyvalueCacheVacancyFill>(Bare::Neither)
        }
        HostFunctionName::P3KeyvalueTypesIncomingValueConsumeAsync => {
            pair::<host_functions::P3KeyvalueTypesIncomingValueConsumeAsync>(Bare::Neither)
        }
        HostFunctionName::P3BlobstoreTypesIncomingValueConsumeAsync => {
            pair::<host_functions::P3BlobstoreTypesIncomingValueConsumeAsync>(Bare::Neither)
        }
        HostFunctionName::P3SystemClockNow => {
            pair::<host_functions::P3SystemClockNow>(Bare::Neither)
        }
        HostFunctionName::P3SystemClockGetResolution => {
            pair::<host_functions::P3SystemClockGetResolution>(Bare::Neither)
        }
        HostFunctionName::P3MonotonicClockNow => {
            pair::<host_functions::P3MonotonicClockNow>(Bare::Neither)
        }
        HostFunctionName::P3MonotonicClockGetResolution => {
            pair::<host_functions::P3MonotonicClockGetResolution>(Bare::Neither)
        }
        HostFunctionName::P3MonotonicClockWaitUntil => {
            pair::<host_functions::P3MonotonicClockWaitUntil>(Bare::Neither)
        }
        HostFunctionName::P3RandomRandomGetRandomBytes => {
            pair::<host_functions::P3RandomRandomGetRandomBytes>(Bare::Neither)
        }
        HostFunctionName::P3RandomRandomGetRandomU64 => {
            pair::<host_functions::P3RandomRandomGetRandomU64>(Bare::Neither)
        }
        HostFunctionName::P3RandomInsecureGetInsecureRandomBytes => {
            pair::<host_functions::P3RandomInsecureGetInsecureRandomBytes>(Bare::Neither)
        }
        HostFunctionName::P3RandomInsecureGetInsecureRandomU64 => {
            pair::<host_functions::P3RandomInsecureGetInsecureRandomU64>(Bare::Neither)
        }
        HostFunctionName::P3RandomInsecureSeedGetInsecureSeed => {
            pair::<host_functions::P3RandomInsecureSeedGetInsecureSeed>(Bare::Neither)
        }
        HostFunctionName::P3FilesystemTypesDescriptorStat => {
            pair::<host_functions::P3FilesystemTypesDescriptorStat>(Bare::Neither)
        }
        HostFunctionName::P3FilesystemTypesDescriptorStatAt => {
            pair::<host_functions::P3FilesystemTypesDescriptorStatAt>(Bare::Neither)
        }
        HostFunctionName::P3SocketsTypesTcpSocketSend => {
            pair::<host_functions::P3SocketsTypesTcpSocketSend>(Bare::Neither)
        }
        HostFunctionName::P3SocketsTypesTcpSocketReceive => {
            pair::<host_functions::P3SocketsTypesTcpSocketReceive>(Bare::Neither)
        }
        HostFunctionName::P3SocketsTypesTcpSocketReceiveChunk => {
            pair::<host_functions::P3SocketsTypesTcpSocketReceiveChunk>(Bare::Neither)
        }
        HostFunctionName::P3SocketsTypesTcpSocketSendAcquire => {
            pair::<host_functions::P3SocketsTypesTcpSocketSendAcquire>(Bare::Neither)
        }
        HostFunctionName::P3SocketsTypesTcpSocketReceiveAcquire => {
            pair::<host_functions::P3SocketsTypesTcpSocketReceiveAcquire>(Bare::Neither)
        }
        HostFunctionName::P3SocketsTypesUdpSocketSend => {
            pair::<host_functions::P3SocketsTypesUdpSocketSend>(Bare::Neither)
        }
        HostFunctionName::P3SocketsTypesUdpSocketReceive => {
            pair::<host_functions::P3SocketsTypesUdpSocketReceive>(Bare::Neither)
        }
        HostFunctionName::P3HttpClientSend => {
            pair::<host_functions::P3HttpClientSend>(Bare::Neither)
        }
        HostFunctionName::P3HttpClientConsumeBody => {
            pair::<host_functions::P3HttpClientConsumeBody>(Bare::Neither)
        }
        HostFunctionName::P3HttpClientConsumeBodyChunk => {
            pair::<host_functions::P3HttpClientConsumeBodyChunk>(Bare::Neither)
        }
        HostFunctionName::P3HttpClientRequestBodyTransmission => {
            pair::<host_functions::P3HttpClientRequestBodyTransmission>(Bare::Neither)
        }
        HostFunctionName::GolemSecretsReveal => {
            pair::<host_functions::GolemSecretsReveal>(Bare::Neither)
        }
        HostFunctionName::GolemApiDeriveCard => {
            pair::<host_functions::GolemApiDeriveCard>(Bare::Neither)
        }
        HostFunctionName::GolemApiInstallCard => {
            pair::<host_functions::GolemApiInstallCard>(Bare::Neither)
        }
        HostFunctionName::FilesystemInputStreamRead => {
            pair::<host_functions::FilesystemInputStreamRead>(Bare::Neither)
        }
        HostFunctionName::FilesystemInputStreamSkip => {
            pair::<host_functions::FilesystemInputStreamSkip>(Bare::Neither)
        }
        HostFunctionName::FilesystemOutputStreamCheckWrite => {
            pair::<host_functions::FilesystemOutputStreamCheckWrite>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsDerivePersist => {
            pair::<host_functions::GolemPermissionsDerivePersist>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsRevokePersist => {
            pair::<host_functions::GolemPermissionsRevokePersist>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsCardLiveness => {
            pair::<host_functions::GolemPermissionsCardLiveness>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsInstallChildPersist => {
            pair::<host_functions::GolemPermissionsInstallChildPersist>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsInstallTransfer => {
            pair::<host_functions::GolemPermissionsInstallTransfer>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsInspectDecision => {
            pair::<host_functions::GolemPermissionsInspectDecision>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsDeriveDecision => {
            pair::<host_functions::GolemPermissionsDeriveDecision>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsDeriveFromWalletDecision => {
            pair::<host_functions::GolemPermissionsDeriveFromWalletDecision>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsDeriveScopeDecision => {
            pair::<host_functions::GolemPermissionsDeriveScopeDecision>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsInstallDecision => {
            pair::<host_functions::GolemPermissionsInstallDecision>(Bare::Neither)
        }
        HostFunctionName::GolemToolGetAllTools => {
            pair::<host_functions::GolemToolGetAllTools>(Bare::Neither)
        }
        HostFunctionName::GolemToolGetTool => {
            pair::<host_functions::GolemToolGetTool>(Bare::Neither)
        }
        HostFunctionName::RdbmsMysqlDbConnectionBeginTransaction => {
            pair::<host_functions::RdbmsMysqlDbConnectionBeginTransaction>(Bare::Neither)
        }
        HostFunctionName::RdbmsPostgresDbConnectionBeginTransaction => {
            pair::<host_functions::RdbmsPostgresDbConnectionBeginTransaction>(Bare::Neither)
        }
        HostFunctionName::RdbmsIgnite2DbConnectionBeginTransaction => {
            pair::<host_functions::RdbmsIgnite2DbConnectionBeginTransaction>(Bare::Neither)
        }
        HostFunctionName::GolemApiGetOplogNext => {
            pair::<host_functions::GolemApiGetOplogNext>(Bare::Neither)
        }
        HostFunctionName::GolemApiSearchOplogNext => {
            pair::<host_functions::GolemApiSearchOplogNext>(Bare::Neither)
        }
        HostFunctionName::GolemApiEnrichOplogEntries => {
            pair::<host_functions::GolemApiEnrichOplogEntries>(Bare::Neither)
        }
        HostFunctionName::WasiConfigGet => pair::<host_functions::WasiConfigGet>(Bare::Neither),
        HostFunctionName::WasiConfigGetAll => {
            pair::<host_functions::WasiConfigGetAll>(Bare::Neither)
        }
        HostFunctionName::P3FilesystemTypesDescriptorWriteViaStream => {
            pair::<host_functions::P3FilesystemTypesDescriptorWriteViaStream>(Bare::Neither)
        }
        HostFunctionName::P3FilesystemTypesDescriptorAppendViaStream => {
            pair::<host_functions::P3FilesystemTypesDescriptorAppendViaStream>(Bare::Neither)
        }
        HostFunctionName::P3SocketsTypesTcpSocketConnect => {
            pair::<host_functions::P3SocketsTypesTcpSocketConnect>(Bare::Neither)
        }
        HostFunctionName::P3SocketsTypesUdpSocketConnect => {
            pair::<host_functions::P3SocketsTypesUdpSocketConnect>(Bare::Neither)
        }
        HostFunctionName::GolemToolRpcInvokeAndAwait => {
            pair::<host_functions::GolemToolRpcInvokeAndAwait>(Bare::Neither)
        }
        HostFunctionName::GolemToolRpcInvoke => {
            pair::<host_functions::GolemToolRpcInvoke>(Bare::Neither)
        }
        HostFunctionName::GolemToolRpcAsyncInvokeAndAwait => {
            pair::<host_functions::GolemToolRpcAsyncInvokeAndAwait>(Bare::Neither)
        }
        HostFunctionName::GolemApiGetAgents => {
            pair::<host_functions::GolemApiGetAgents>(Bare::Neither)
        }
        HostFunctionName::WasiCliEnvironmentGetEnvironment => {
            pair::<host_functions::WasiCliEnvironmentGetEnvironment>(Bare::Neither)
        }
        HostFunctionName::GolemRpcWasmRpcActivate => {
            pair::<host_functions::GolemRpcWasmRpcActivate>(Bare::Neither)
        }
        HostFunctionName::GolemToolResponseSecretHoldAdmission => {
            pair::<host_functions::GolemToolResponseSecretHoldAdmission>(Bare::Neither)
        }
        HostFunctionName::GolemToolAwaitInvokeResults => {
            pair::<host_functions::GolemToolAwaitInvokeResults>(Bare::Neither)
        }
        HostFunctionName::GolemEntityInvoke => {
            pair::<host_functions::GolemEntityInvoke>(Bare::Both)
        }
        HostFunctionName::GolemToolInvocationRejected => {
            pair::<host_functions::GolemToolInvocationRejected>(Bare::Response)
        }
        HostFunctionName::McpToolCall => pair::<host_functions::McpToolCall>(Bare::Request),
        HostFunctionName::McpToolPresence => pair::<host_functions::McpToolPresence>(Bare::Neither),
        HostFunctionName::GolemAgentGetAgentTypeByAgentId => {
            pair::<host_functions::GolemAgentGetAgentTypeByAgentId>(Bare::Neither)
        }
        HostFunctionName::GolemToolCreateStdin => {
            pair::<host_functions::GolemToolCreateStdin>(Bare::Neither)
        }
        HostFunctionName::GolemToolCreateOutput => {
            pair::<host_functions::GolemToolCreateOutput>(Bare::Neither)
        }
        HostFunctionName::GolemToolStdinWrite => {
            pair::<host_functions::GolemToolStdinWrite>(Bare::Neither)
        }
        HostFunctionName::GolemToolStdinFinish => {
            pair::<host_functions::GolemToolStdinFinish>(Bare::Neither)
        }
        HostFunctionName::GolemToolStdinFail => {
            pair::<host_functions::GolemToolStdinFail>(Bare::Neither)
        }
        HostFunctionName::GolemToolStdinWait => {
            pair::<host_functions::GolemToolStdinWait>(Bare::Neither)
        }
        HostFunctionName::GolemToolOutputWrite => {
            pair::<host_functions::GolemToolOutputWrite>(Bare::Neither)
        }
        HostFunctionName::GolemToolOutputFinish => {
            pair::<host_functions::GolemToolOutputFinish>(Bare::Neither)
        }
        HostFunctionName::GolemToolOutputFail => {
            pair::<host_functions::GolemToolOutputFail>(Bare::Neither)
        }
        HostFunctionName::FilesystemPermissionCheck => {
            pair::<host_functions::FilesystemPermissionCheck>(Bare::Neither)
        }
        HostFunctionName::SocketsTcpConnectPermissionCheck => {
            pair::<host_functions::SocketsTcpConnectPermissionCheck>(Bare::Neither)
        }
        HostFunctionName::SocketsUdpPermissionCheck => {
            pair::<host_functions::SocketsUdpPermissionCheck>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsSelfWallet => {
            pair::<host_functions::GolemPermissionsSelfWallet>(Bare::Neither)
        }
        HostFunctionName::GolemPermissionsSelfVersion => {
            pair::<host_functions::GolemPermissionsSelfVersion>(Bare::Neither)
        }
    }
}

static PAIR_GRAPHS: LazyLock<Mutex<HashMap<String, Option<Arc<PairGraphs>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn trusted_graph(function: &str, direction: Direction) -> Option<SchemaGraph> {
    let graphs = {
        let mut cache = PAIR_GRAPHS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache
            .entry(function.to_string())
            .or_insert_with(|| pair_graphs(&HostFunctionName::from(function)).map(Arc::new))
            .clone()
    }?;
    match direction {
        Direction::Request => graphs.request.clone(),
        Direction::Response => graphs.response.clone(),
    }
}

/// Root type IDs of the payloads that have their own rule nodes.
struct RuleIds {
    rpc_invoke: TypeId,
    rpc_scheduled_invocation: TypeId,
    config_value: TypeId,
    fork_agent: TypeId,
    random_bytes: TypeId,
    invoke_and_await: TypeId,
    agent_types: TypeId,
    agent_type: TypeId,
}

static RULE_IDS: LazyLock<RuleIds> = LazyLock::new(|| RuleIds {
    rpc_invoke: <HostRequestGolemRpcInvoke as IntoSchema>::type_id(),
    rpc_scheduled_invocation: <HostRequestGolemRpcScheduledInvocation as IntoSchema>::type_id(),
    config_value: <HostRequestGolemAgentGetConfigValue as IntoSchema>::type_id(),
    fork_agent: <HostRequestGolemApiForkAgent as IntoSchema>::type_id(),
    random_bytes: <HostRequestRandomBytes as IntoSchema>::type_id(),
    invoke_and_await: <HostResponseGolemRpcInvokeAndAwait as IntoSchema>::type_id(),
    agent_types: <HostResponseGolemAgentAgentTypes as IntoSchema>::type_id(),
    agent_type: <HostResponseGolemAgentAgentType as IntoSchema>::type_id(),
});

impl Walker<'_, '_> {
    /// Renders a host payload. `function` is the host function name of the `Start`, or `None`
    /// when it is unknown. Returns the node and whether the payload was trusted.
    pub(super) fn host_payload(
        &mut self,
        function: Option<&str>,
        direction: Direction,
        value: &TypedSchemaValue,
    ) -> (ReportNode, bool) {
        let trusted = function
            .and_then(|function| trusted_graph(function, direction))
            .is_some_and(|expected| &expected == value.graph());
        if !trusted {
            return (self.customer(value), false);
        }
        (
            self.trusted_root(value.graph(), value.root_type(), value.value()),
            true,
        )
    }

    fn trusted_root(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
    ) -> ReportNode {
        let Some(resolved) = resolve(graph, ty) else {
            return self.schemaless(value, Region::Customer);
        };
        let (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: values }) =
            (resolved.body, value)
        else {
            return self.typed(graph, ty, value, Region::Trusted);
        };
        if fields.len() != values.len() {
            return self.schemaless(value, Region::Customer);
        }
        let root_id = resolved.ids.first().copied();
        let ids = &*RULE_IDS;
        let is = |id: &TypeId| root_id == Some(id);
        let fields = fields
            .iter()
            .zip(values)
            .map(|(field, value)| {
                let name = normalized(&field.name);
                let body = &field.body;
                let node = if (is(&ids.rpc_invoke) || is(&ids.rpc_scheduled_invocation))
                    && name == "methodname"
                {
                    self.kept_string(graph, body, value)
                } else if (is(&ids.rpc_invoke) || is(&ids.rpc_scheduled_invocation))
                    && name == "input"
                {
                    self.rpc_value("request.input", value)
                } else if is(&ids.config_value) && name == "path" {
                    self.kept_string_list(graph, body, value)
                } else if (is(&ids.fork_agent) && name == "oplogindexcutoff")
                    || (is(&ids.random_bytes) && name == "length")
                {
                    match unsigned(value) {
                        Some(n) => number(n),
                        None => self.typed(graph, body, value, Region::Trusted),
                    }
                } else if is(&ids.invoke_and_await) && name == "result" {
                    self.kept_result(graph, body, value, |walker, _, _, ok| {
                        walker.rpc_value("response.result.ok", ok)
                    })
                } else if (is(&ids.agent_types) || is(&ids.agent_type)) && name == "result" {
                    self.kept_result(graph, body, value, |walker, graph, ty, ok| {
                        walker.kept_container(graph, ty, ok)
                    })
                } else if name == "result" && is_result(graph, body) {
                    self.kept_result(graph, body, value, |walker, graph, ty, ok| {
                        walker.typed(graph, ty, ok, Region::Trusted)
                    })
                } else {
                    self.typed(graph, body, value, Region::Trusted)
                };
                ReportField {
                    name: Some(field.name.clone()),
                    value: node,
                }
            })
            .collect();
        record(resolved.type_name, fields)
    }

    fn kept_string(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
    ) -> ReportNode {
        match value {
            SchemaValue::String(s) => text(s.clone()),
            _ => self.typed(graph, ty, value, Region::Trusted),
        }
    }

    fn kept_string_list(
        &mut self,
        graph: &SchemaGraph,
        ty: &SchemaType,
        value: &SchemaValue,
    ) -> ReportNode {
        match value {
            SchemaValue::List { elements } => {
                let names: Option<Vec<ReportNode>> = elements
                    .iter()
                    .map(|element| match element {
                        SchemaValue::String(name) => Some(text(name.clone())),
                        _ => None,
                    })
                    .collect();
                match names {
                    Some(elements) => ReportNode::Tuple(TupleNode { elements }),
                    None => self.typed(graph, ty, value, Region::Trusted),
                }
            }
            _ => self.typed(graph, ty, value, Region::Trusted),
        }
    }
}

fn is_result(graph: &SchemaGraph, ty: &SchemaType) -> bool {
    matches!(
        resolve(graph, ty).map(|resolved| resolved.body),
        Some(SchemaType::Result { .. })
    )
}
