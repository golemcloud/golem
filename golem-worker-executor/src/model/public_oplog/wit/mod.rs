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

#[macro_use]
mod entry_conversion;
mod cards;
mod public_entries;
mod raw_entries;
mod retry;
mod snapshots;
mod tracing;
mod values;

use cards::{
    card_id_from_wit, card_id_to_wit, card_install_failure_from_wit, card_install_failure_to_wit,
    invocation_wallet_pin_from_wit, invocation_wallet_pin_to_wit,
    public_invocation_wallet_pin_to_wit, queued_card_event_to_wit, raw_queued_card_event_from_wit,
    raw_queued_card_event_to_wit,
};
use snapshots::multipart_to_raw;
use tracing::{
    log_trace_context_from_wit, log_trace_context_to_wit, public_span_attributes_to_wit,
    public_span_finished_to_wit, public_span_started_to_wit, raw_span_attributes_from_wit,
    raw_span_attributes_to_wit, raw_span_finished_from_wit, raw_span_finished_to_wit,
    raw_span_started_from_wit, raw_span_started_to_wit,
};
pub(crate) use values::reject_quota_handles_in_oplog_entries;
use values::{
    decode_untyped_schema_value, encode_public_typed_schema_value, encode_untyped_schema_value,
    oplog_payload_from_wit, oplog_payload_to_wit,
};

#[cfg(test)]
mod tests;

use crate::preview2::golem_api_1_x::oplog;
use golem_common::base_model::oplog::{
    CardInstallFailure, PublicQueuedCardEvent, QueuedCardEvent, QueuedCardEventCard,
};
use golem_common::model::card::{
    CardId, InvocationWalletPin, PublicInvocationWalletPin, WalletVersionToken,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::payload::types::{SerializableToolError, SerializableToolRpcError};
use golem_common::model::oplog::public_oplog_entry::{
    ActivatePluginParams, AgentInvocationFinishedParams, AgentInvocationStartedParams,
    BeginRemoteTransactionParams, CancelPendingInvocationParams, CancelledParams,
    CardEventQueuedParams, CardExpiredParams, CardInstallFailedParams, CardInstalledParams,
    CardRevokedParams, CommittedRemoteTransactionParams, CompletionDeliveredParams,
    CompletionDiscardedParams, CreateParams, CreateResourceParams, DeactivatePluginParams,
    DropResourceParams, EndParams, ErrorParams, ExitedParams, FailedUpdateParams, GrowMemoryParams,
    HostStreamFrameParams, InterruptedParams, JumpParams, LogParams, ManualUpdateParameters,
    OplogProcessorCheckpointParams, PendingAgentInvocationParams, PendingUpdateParams,
    PluginInstallationDescription, PreCommitRemoteTransactionParams,
    PreRollbackRemoteTransactionParams, PublicAgentInvocation, PublicAgentInvocationResult,
    PublicAttribute, PublicAttributeValue, PublicDurableFunctionType, PublicExternalToolResult,
    PublicSpanAttributes, PublicSpanData, PublicSpanFinished, PublicSpanKind, PublicSpanOutcome,
    PublicSpanStarted, RecoverySucceededParams, RemoveRetryPolicyParams, RestartParams,
    ResumedParams, RevertParams, RolledBackRemoteTransactionParams, SetRetryPolicyParams,
    SnapshotConfirmedParams, SnapshotParams, SpanClosedParams, StartParams, StreamCancelParams,
    StreamEndParams, StreamItemsParams, StreamRegisteredParams, StreamSessionParams,
    StringAttributeValue, SuccessfulUpdateParams, SuspendParams, ToolAttachmentControlParams,
    ToolAttachmentReadParams,
};

use golem_common::model::oplog::{
    AgentInvocationOutputParameters, AgentTerminatedByQuotaError, EphemeralCannotSuspendError,
    EphemeralFuelExhaustedError, EphemeralSleepTooLongError, FallibleResultParameters,
    JsonSnapshotData, MultipartPartData, MultipartSnapshotData, PublicOplogEntry,
    PublicSnapshotData, PublicUpdateDescription, RawSnapshotData, ReadOnlyViolationError,
    SaveSnapshotResultParameters, SnapshotBasedUpdateParameters,
};
use golem_common::model::quota::ResourceName;
use golem_common::model::{Empty, Timestamp};
use golem_common::schema::{
    SchemaValue, TypedSchemaValue, redact_host_managed_typed_value, redact_host_managed_value,
};
use golem_schema::schema::wit::{
    PermissionCardHandleDropper, QuotaTokenHandleDropper, SecretHandleDropper, decode_value,
    encode_typed, encode_value, reject_quota_handles_in_value_tree, wire,
};

fn durable_invocation_id_to_wit(
    id: golem_common::base_model::oplog::DurableInvocationId,
) -> oplog::DurableInvocationId {
    oplog::DurableInvocationId {
        anchor: id.anchor.as_u64(),
        ordinal: id.ordinal,
    }
}

fn durable_invocation_id_from_wit(
    id: oplog::DurableInvocationId,
) -> golem_common::base_model::oplog::DurableInvocationId {
    golem_common::base_model::oplog::DurableInvocationId::new(
        golem_common::base_model::OplogIndex::from_u64(id.anchor),
        id.ordinal,
    )
}

fn durable_stream_summary_to_wit(
    value: golem_common::model::oplog::DurableStreamEventSummary,
) -> oplog::DurableStreamEventSummary {
    use golem_common::model::oplog::DurableStreamEventSummary as S;
    let outcome = |value| match value {
        golem_common::model::oplog::DurableStreamOutcome::Success => {
            oplog::DurableStreamOutcome::Success
        }
        golem_common::model::oplog::DurableStreamOutcome::Error => {
            oplog::DurableStreamOutcome::Error
        }
    };
    match value {
        S::Registered => oplog::DurableStreamEventSummary::Registered,
        S::Items { item_count } => oplog::DurableStreamEventSummary::Items(item_count),
        S::End { outcome: value } => oplog::DurableStreamEventSummary::End(outcome(value)),
        S::Cancelled => oplog::DurableStreamEventSummary::Cancelled,
        S::SessionResult => oplog::DurableStreamEventSummary::SessionResult,
        S::SessionFinished { outcome: value } => {
            oplog::DurableStreamEventSummary::SessionFinished(outcome(value))
        }
        S::SessionCancellation => oplog::DurableStreamEventSummary::SessionCancellation,
        S::SessionExpired => oplog::DurableStreamEventSummary::SessionExpired,
    }
}

fn durable_stream_summary_from_wit(
    value: oplog::DurableStreamEventSummary,
) -> golem_common::model::oplog::DurableStreamEventSummary {
    use golem_common::model::oplog::{DurableStreamEventSummary as S, DurableStreamOutcome as O};
    let outcome = |value| match value {
        oplog::DurableStreamOutcome::Success => O::Success,
        oplog::DurableStreamOutcome::Error => O::Error,
    };
    match value {
        oplog::DurableStreamEventSummary::Registered => S::Registered,
        oplog::DurableStreamEventSummary::Items(count) => S::Items { item_count: count },
        oplog::DurableStreamEventSummary::End(value) => S::End {
            outcome: outcome(value),
        },
        oplog::DurableStreamEventSummary::Cancelled => S::Cancelled,
        oplog::DurableStreamEventSummary::SessionResult => S::SessionResult,
        oplog::DurableStreamEventSummary::SessionFinished(value) => S::SessionFinished {
            outcome: outcome(value),
        },
        oplog::DurableStreamEventSummary::SessionCancellation => S::SessionCancellation,
        oplog::DurableStreamEventSummary::SessionExpired => S::SessionExpired,
    }
}

impl From<golem_common::model::oplog::HostStreamKind> for oplog::HostStreamKind {
    fn from(value: golem_common::model::oplog::HostStreamKind) -> Self {
        match value {
            golem_common::model::oplog::HostStreamKind::P3HttpRequestBody => {
                Self::P3HttpRequestBody
            }
        }
    }
}

impl From<golem_common::model::oplog::OplogErrorKind> for oplog::OplogErrorKind {
    fn from(value: golem_common::model::oplog::OplogErrorKind) -> Self {
        match value {
            golem_common::model::oplog::OplogErrorKind::Invocation => Self::Invocation,
            golem_common::model::oplog::OplogErrorKind::Recovery => Self::Recovery,
        }
    }
}

impl From<oplog::OplogErrorKind> for golem_common::model::oplog::OplogErrorKind {
    fn from(value: oplog::OplogErrorKind) -> Self {
        match value {
            oplog::OplogErrorKind::Invocation => Self::Invocation,
            oplog::OplogErrorKind::Recovery => Self::Recovery,
        }
    }
}

impl From<oplog::HostStreamKind> for golem_common::model::oplog::HostStreamKind {
    fn from(value: oplog::HostStreamKind) -> Self {
        match value {
            oplog::HostStreamKind::P3HttpRequestBody => Self::P3HttpRequestBody,
        }
    }
}

impl From<PublicDurableFunctionType> for oplog::WrappedFunctionType {
    fn from(value: PublicDurableFunctionType) -> Self {
        match value {
            PublicDurableFunctionType::WriteLocal(_) => Self::WriteLocal,
            PublicDurableFunctionType::ReadLocal(_) => Self::ReadLocal,
            PublicDurableFunctionType::WriteRemote(_) => Self::WriteRemote,
            PublicDurableFunctionType::ReadRemote(_) => Self::ReadRemote,
        }
    }
}

impl From<golem_common::model::oplog::LogLevel> for oplog::LogLevel {
    fn from(value: golem_common::model::oplog::LogLevel) -> Self {
        match value {
            golem_common::model::oplog::LogLevel::Trace => Self::Trace,
            golem_common::model::oplog::LogLevel::Debug => Self::Debug,
            golem_common::model::oplog::LogLevel::Info => Self::Info,
            golem_common::model::oplog::LogLevel::Warn => Self::Warn,
            golem_common::model::oplog::LogLevel::Error => Self::Error,
            golem_common::model::oplog::LogLevel::Critical => Self::Critical,
            golem_common::model::oplog::LogLevel::Stdout => Self::Stdout,
            golem_common::model::oplog::LogLevel::Stderr => Self::Stderr,
        }
    }
}

impl From<PluginInstallationDescription> for oplog::PluginInstallationDescription {
    fn from(value: PluginInstallationDescription) -> Self {
        Self {
            environment_plugin_grant_id: oplog::EnvironmentPluginGrantId {
                uuid: value.environment_plugin_grant_id.0.into(),
            },
            plugin_priority: value.plugin_priority.0,
            plugin_name: value.plugin_name,
            plugin_version: value.plugin_version,
            parameters: value.parameters.into_iter().collect(),
        }
    }
}

impl From<EnvironmentId> for oplog::EnvironmentId {
    fn from(value: EnvironmentId) -> Self {
        Self {
            uuid: value.0.into(),
        }
    }
}

impl From<Timestamp> for oplog::Timestamp {
    fn from(value: Timestamp) -> Self {
        oplog::Timestamp {
            timestamp: value.into(),
        }
    }
}

fn timestamp_from_datetime(
    dt: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
) -> Timestamp {
    Timestamp::from(dt.seconds.max(0) as u64 * 1000 + (dt.nanoseconds / 1_000_000) as u64)
}

impl From<oplog::WorkerError> for golem_common::model::oplog::AgentError {
    fn from(err: oplog::WorkerError) -> Self {
        match err {
            oplog::WorkerError::Unknown(msg) => Self::Unknown(msg),
            oplog::WorkerError::InvalidRequest(msg) => Self::InvalidRequest(msg),
            oplog::WorkerError::StackOverflow => Self::StackOverflow,
            oplog::WorkerError::OutOfMemory => Self::OutOfMemory,
            oplog::WorkerError::ExceededMemoryLimit => Self::ExceededMemoryLimit,
            oplog::WorkerError::InternalError(msg) => Self::InternalError(msg),
            oplog::WorkerError::DeterministicTrap(msg) => Self::DeterministicTrap(msg),
            oplog::WorkerError::TransientError(msg) => Self::TransientError(msg),
            oplog::WorkerError::PermanentError(msg) => Self::PermanentError(msg),
            oplog::WorkerError::ExceededTableLimit => Self::ExceededTableLimit,
            oplog::WorkerError::ExceededHttpCallLimit => Self::ExceededHttpCallLimit,
            oplog::WorkerError::ExceededRpcCallLimit => Self::ExceededRpcCallLimit,
            oplog::WorkerError::AgentTerminatedByQuota(inner) => {
                Self::AgentTerminatedByQuota(AgentTerminatedByQuotaError {
                    environment_id: EnvironmentId(inner.environment_id.uuid.into()),
                    resource_name: ResourceName(inner.resource_name),
                })
            }
            oplog::WorkerError::EphemeralSleepTooLong(inner) => {
                Self::EphemeralSleepTooLong(EphemeralSleepTooLongError {
                    requested_nanos: inner.requested_nanos,
                    max_nanos: inner.max_nanos,
                })
            }
            oplog::WorkerError::EphemeralFuelExhausted(inner) => {
                Self::EphemeralFuelExhausted(EphemeralFuelExhaustedError {
                    overdraft_limit: inner.overdraft_limit,
                })
            }
            oplog::WorkerError::EphemeralCannotSuspend(inner) => {
                Self::EphemeralCannotSuspend(EphemeralCannotSuspendError {
                    reason: inner.reason,
                })
            }
            oplog::WorkerError::ReadOnlyViolation(inner) => {
                Self::ReadOnlyViolation(ReadOnlyViolationError {
                    method: inner.method,
                    host_function: inner.host_function,
                })
            }
        }
    }
}

// Note: From<oplog::WrappedFunctionType> for DurableFunctionType is provided by golem_common's derive macros

impl From<oplog::LogLevel> for golem_common::model::oplog::LogLevel {
    fn from(level: oplog::LogLevel) -> Self {
        match level {
            oplog::LogLevel::Trace => Self::Trace,
            oplog::LogLevel::Debug => Self::Debug,
            oplog::LogLevel::Info => Self::Info,
            oplog::LogLevel::Warn => Self::Warn,
            oplog::LogLevel::Error => Self::Error,
            oplog::LogLevel::Critical => Self::Critical,
            oplog::LogLevel::Stdout => Self::Stdout,
            oplog::LogLevel::Stderr => Self::Stderr,
        }
    }
}

impl From<golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId>
    for oplog::EnvironmentPluginGrantId
{
    fn from(
        value: golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId,
    ) -> Self {
        Self {
            uuid: value.0.into(),
        }
    }
}

impl From<golem_common::model::oplog::AgentError> for oplog::WorkerError {
    fn from(err: golem_common::model::oplog::AgentError) -> Self {
        use golem_common::model::oplog::AgentError;
        match err {
            AgentError::Unknown(msg) => Self::Unknown(msg),
            AgentError::InvalidRequest(msg) => Self::InvalidRequest(msg),
            AgentError::StackOverflow => Self::StackOverflow,
            AgentError::OutOfMemory => Self::OutOfMemory,
            AgentError::ExceededMemoryLimit => Self::ExceededMemoryLimit,
            AgentError::InternalError(msg) => Self::InternalError(msg),
            AgentError::DeterministicTrap(msg) => Self::DeterministicTrap(msg),
            AgentError::TransientError(msg) => Self::TransientError(msg),
            AgentError::PermanentError(msg) => Self::PermanentError(msg),
            AgentError::ExceededTableLimit => Self::ExceededTableLimit,
            AgentError::ExceededHttpCallLimit => Self::ExceededHttpCallLimit,
            AgentError::ExceededRpcCallLimit => Self::ExceededRpcCallLimit,
            AgentError::AgentTerminatedByQuota(inner) => {
                Self::AgentTerminatedByQuota(oplog::AgentTerminatedByQuotaError {
                    environment_id: inner.environment_id.into(),
                    resource_name: inner.resource_name.0,
                })
            }
            AgentError::EphemeralSleepTooLong(inner) => {
                Self::EphemeralSleepTooLong(oplog::EphemeralSleepTooLong {
                    requested_nanos: inner.requested_nanos,
                    max_nanos: inner.max_nanos,
                })
            }
            AgentError::EphemeralFuelExhausted(inner) => {
                Self::EphemeralFuelExhausted(oplog::EphemeralFuelExhausted {
                    overdraft_limit: inner.overdraft_limit,
                })
            }
            AgentError::EphemeralCannotSuspend(inner) => {
                Self::EphemeralCannotSuspend(oplog::EphemeralCannotSuspend {
                    reason: inner.reason,
                })
            }
            AgentError::ReadOnlyViolation(inner) => {
                Self::ReadOnlyViolation(oplog::ReadOnlyViolation {
                    method: inner.method,
                    host_function: inner.host_function,
                })
            }
            AgentError::PermissionDenied(message) => Self::PermanentError(message),
        }
    }
}

// Note: From<DurableFunctionType> for oplog::WrappedFunctionType is already provided in
// durable_host/durability.rs (as `From<DurableFunctionType> for durability::DurableFunctionType`,
// where `durability::DurableFunctionType` is the same bindgen type as `oplog::WrappedFunctionType`).
