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

use super::spans::{decode_public_span_data, encode_public_span_data};
use crate::base_model::OplogIndex;
use crate::model::invocation_context::TraceId;
use crate::model::oplog::payload::types::{
    SerializableCustomToolError, SerializableToolError, SerializableToolInvocationResult,
    SerializableToolRpcError,
};
use crate::model::oplog::{
    AgentError, AgentInitializationParameters, AgentInvocationOutputParameters,
    AgentMethodInvocationParameters, AgentTerminatedByQuotaError, EphemeralCannotSuspendError,
    EphemeralFuelExhaustedError, EphemeralSleepTooLongError, ExternalToolInvocationParameters,
    ExternalToolResultParameters, FallibleResultParameters, JsonSnapshotData,
    LoadSnapshotParameters, ManualUpdateParameters, MultipartPartData, MultipartSnapshotData,
    MultipartSnapshotPart, ProcessOplogEntriesParameters, ProcessOplogEntriesResultParameters,
    PublicAgentEntity, PublicAgentEntityKind, PublicAgentInvocation, PublicAgentInvocationResult,
    PublicEntityCallMode, PublicEntityInvocation, PublicEntityInvocationContext,
    PublicEntityInvocationOperation, PublicExternalToolResult, PublicSnapshotData,
    PublicToolInvocationOperation, RawSnapshotData, ReadOnlyViolationError,
    SaveSnapshotResultParameters,
};
use crate::model::quota::ResourceName;
use crate::model::{AgentInvocationResult, Empty};

impl TryFrom<golem_api_grpc::proto::golem::worker::AgentError> for AgentError {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::AgentError,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::agent_error::Error;
        match value.error.ok_or("no error field")? {
            Error::StackOverflow(_) => Ok(Self::StackOverflow),
            Error::OutOfMemory(_) => Ok(Self::OutOfMemory),
            Error::InvalidRequest(inner) => Ok(Self::InvalidRequest(inner.details)),
            Error::UnknownError(inner) => Ok(Self::Unknown(inner.details)),
            Error::ExceededMemoryLimit(_) => Ok(Self::ExceededMemoryLimit),
            Error::InternalError(inner) => Ok(Self::InternalError(inner.details)),
            Error::DeterministicTrap(inner) => Ok(Self::DeterministicTrap(inner.details)),
            Error::TransientError(inner) => Ok(Self::TransientError(inner.details)),
            Error::PermanentError(inner) => Ok(Self::PermanentError(inner.details)),
            Error::ExceededTableLimit(_) => Ok(Self::ExceededTableLimit),
            Error::ExceededHttpCallLimit(_) => Ok(Self::ExceededHttpCallLimit),
            Error::ExceededRpcCallLimit(_) => Ok(Self::ExceededRpcCallLimit),
            Error::AgentTerminatedByQuota(inner) => {
                Ok(Self::AgentTerminatedByQuota(AgentTerminatedByQuotaError {
                    environment_id: inner
                        .environment_id
                        .ok_or("no environment_id field")?
                        .try_into()?,
                    resource_name: ResourceName(inner.resource_name),
                }))
            }
            Error::EphemeralSleepTooLong(inner) => {
                Ok(Self::EphemeralSleepTooLong(EphemeralSleepTooLongError {
                    requested_nanos: inner.requested_nanos,
                    max_nanos: inner.max_nanos,
                }))
            }
            Error::EphemeralFuelExhausted(inner) => {
                Ok(Self::EphemeralFuelExhausted(EphemeralFuelExhaustedError {
                    overdraft_limit: inner.overdraft_limit,
                }))
            }
            Error::EphemeralCannotSuspend(inner) => {
                Ok(Self::EphemeralCannotSuspend(EphemeralCannotSuspendError {
                    reason: inner.reason,
                }))
            }
            Error::ReadOnlyViolation(inner) => {
                Ok(Self::ReadOnlyViolation(ReadOnlyViolationError {
                    method: inner.method,
                    host_function: inner.host_function,
                }))
            }
            Error::PermissionDenied(inner) => Ok(Self::PermissionDenied(inner.details)),
        }
    }
}

impl From<AgentError> for golem_api_grpc::proto::golem::worker::AgentError {
    fn from(value: AgentError) -> Self {
        use golem_api_grpc::proto::golem::worker as grpc_worker;
        use golem_api_grpc::proto::golem::worker::agent_error::Error;
        let error = match value {
            AgentError::StackOverflow => Error::StackOverflow(grpc_worker::StackOverflow {}),
            AgentError::OutOfMemory => Error::OutOfMemory(grpc_worker::OutOfMemory {}),
            AgentError::InvalidRequest(details) => {
                Error::InvalidRequest(grpc_worker::InvalidRequest { details })
            }
            AgentError::Unknown(details) => {
                Error::UnknownError(grpc_worker::UnknownError { details })
            }
            AgentError::ExceededMemoryLimit => {
                Error::ExceededMemoryLimit(grpc_worker::ExceededMemoryLimit {})
            }
            AgentError::InternalError(details) => {
                Error::InternalError(grpc_worker::InternalError { details })
            }
            AgentError::DeterministicTrap(details) => {
                Error::DeterministicTrap(grpc_worker::DeterministicTrap { details })
            }
            AgentError::TransientError(details) => {
                Error::TransientError(grpc_worker::TransientError { details })
            }
            AgentError::PermanentError(details) => {
                Error::PermanentError(grpc_worker::PermanentError { details })
            }
            AgentError::ExceededTableLimit => {
                Error::ExceededTableLimit(grpc_worker::ExceededTableLimit {})
            }
            AgentError::ExceededHttpCallLimit => {
                Error::ExceededHttpCallLimit(grpc_worker::ExceededHttpCallLimit {})
            }
            AgentError::ExceededRpcCallLimit => {
                Error::ExceededRpcCallLimit(grpc_worker::ExceededRpcCallLimit {})
            }
            AgentError::AgentTerminatedByQuota(details) => {
                Error::AgentTerminatedByQuota(grpc_worker::AgentTerminatedByQuota {
                    environment_id: Some(details.environment_id.into()),
                    resource_name: details.resource_name.0,
                })
            }
            AgentError::EphemeralSleepTooLong(EphemeralSleepTooLongError {
                requested_nanos,
                max_nanos,
            }) => Error::EphemeralSleepTooLong(grpc_worker::EphemeralSleepTooLong {
                requested_nanos,
                max_nanos,
            }),
            AgentError::EphemeralFuelExhausted(EphemeralFuelExhaustedError { overdraft_limit }) => {
                Error::EphemeralFuelExhausted(grpc_worker::EphemeralFuelExhausted {
                    overdraft_limit,
                })
            }
            AgentError::EphemeralCannotSuspend(EphemeralCannotSuspendError { reason }) => {
                Error::EphemeralCannotSuspend(grpc_worker::EphemeralCannotSuspend { reason })
            }
            AgentError::ReadOnlyViolation(ReadOnlyViolationError {
                method,
                host_function,
            }) => Error::ReadOnlyViolation(grpc_worker::ReadOnlyViolation {
                method,
                host_function,
            }),
            AgentError::PermissionDenied(details) => {
                Error::PermissionDenied(grpc_worker::PermissionDenied { details })
            }
        };
        Self { error: Some(error) }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicAgentInvocation>
    for PublicAgentInvocation
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicAgentInvocation,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_agent_invocation::Invocation;
        match value.invocation.ok_or("Missing invocation field")? {
            Invocation::AgentInitialization(init) => {
                let constructor_parameters = init
                    .constructor_parameters
                    .ok_or("Missing constructor_parameters field")?
                    .try_into()?;
                let invocation_context = encode_public_span_data(init.invocation_context)?;
                Ok(PublicAgentInvocation::AgentInitialization(
                    AgentInitializationParameters {
                        idempotency_key: init
                            .idempotency_key
                            .ok_or("Missing idempotency_key field")?
                            .into(),
                        constructor_parameters,
                        trace_id: TraceId::from_string(init.trace_id)?,
                        trace_states: init.trace_states,
                        invocation_context,
                    },
                ))
            }
            Invocation::AgentMethod(method) => {
                let function_input = method
                    .function_input
                    .ok_or("Missing function_input field")?
                    .try_into()?;
                let invocation_context = encode_public_span_data(method.invocation_context)?;
                Ok(PublicAgentInvocation::AgentMethodInvocation(
                    AgentMethodInvocationParameters {
                        idempotency_key: method
                            .idempotency_key
                            .ok_or("Missing idempotency_key field")?
                            .into(),
                        method_name: method.method_name,
                        function_input,
                        trace_id: TraceId::from_string(method.trace_id)?,
                        trace_states: method.trace_states,
                        invocation_context,
                    },
                ))
            }
            Invocation::ExternalTool(tool) => {
                let input = tool.input.ok_or("Missing input field")?.try_into()?;
                let invocation_context = encode_public_span_data(tool.invocation_context)?;
                Ok(PublicAgentInvocation::ExternalTool(
                    ExternalToolInvocationParameters {
                        idempotency_key: tool
                            .idempotency_key
                            .ok_or("Missing idempotency_key field")?
                            .into(),
                        tool_name: tool.tool_name,
                        command_path: tool.command_path,
                        input,
                        trace_id: TraceId::from_string(tool.trace_id)?,
                        trace_states: tool.trace_states,
                        invocation_context,
                    },
                ))
            }
            Invocation::SaveSnapshot(_) => Ok(PublicAgentInvocation::SaveSnapshot(Empty {})),
            Invocation::LoadSnapshot(load) => {
                let snapshot = load.snapshot.ok_or("Missing snapshot field")?;
                let data = match snapshot.data.ok_or("Missing data field")? {
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Raw(raw) => {
                        PublicSnapshotData::Raw(RawSnapshotData {
                            data: raw.data,
                            mime_type: raw.mime_type,
                        })
                    }
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Json(json) => {
                        PublicSnapshotData::Json(JsonSnapshotData {
                            data: serde_json::from_str(&json.data).map_err(|e| e.to_string())?,
                        })
                    }
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Multipart(
                        multipart,
                    ) => PublicSnapshotData::Multipart(MultipartSnapshotData {
                        mime_type: multipart.mime_type,
                        parts: multipart.parts.into_iter().map(|p| {
                            let data = match p.data.and_then(|d| d.data) {
                                Some(golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Json(json)) => {
                                    MultipartPartData::Json(JsonSnapshotData {
                                        data: serde_json::from_str(&json.data).unwrap_or_default(),
                                    })
                                }
                                Some(golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Raw(raw)) => {
                                    MultipartPartData::Raw(RawSnapshotData {
                                        data: raw.data,
                                        mime_type: raw.mime_type,
                                    })
                                }
                                None => MultipartPartData::Raw(RawSnapshotData {
                                    data: vec![],
                                    mime_type: String::new(),
                                }),
                            };
                            MultipartSnapshotPart {
                                name: p.name,
                                content_type: p.content_type,
                                data,
                            }
                        }).collect(),
                    }),
                };
                Ok(PublicAgentInvocation::LoadSnapshot(
                    LoadSnapshotParameters { snapshot: data },
                ))
            }
            Invocation::ProcessOplogEntries(process) => Ok(
                PublicAgentInvocation::ProcessOplogEntries(ProcessOplogEntriesParameters {
                    idempotency_key: process
                        .idempotency_key
                        .ok_or("Missing idempotency_key field")?
                        .into(),
                }),
            ),
            Invocation::ManualUpdate(manual) => Ok(PublicAgentInvocation::ManualUpdate(
                ManualUpdateParameters {
                    target_revision: manual.target_revision.try_into()?,
                },
            )),
        }
    }
}

impl TryFrom<PublicAgentInvocation>
    for golem_api_grpc::proto::golem::worker::PublicAgentInvocation
{
    type Error = String;

    fn try_from(value: PublicAgentInvocation) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_agent_invocation::Invocation;
        let invocation = match value {
            PublicAgentInvocation::AgentInitialization(init) => {
                let invocation_context = decode_public_span_data(&init.invocation_context, 0);
                Invocation::AgentInitialization(
                    golem_api_grpc::proto::golem::worker::PublicAgentInitializationInvocation {
                        idempotency_key: Some(init.idempotency_key.into()),
                        constructor_parameters: Some(init.constructor_parameters.try_into()?),
                        trace_id: init.trace_id.to_string(),
                        trace_states: init.trace_states,
                        invocation_context,
                    },
                )
            }
            PublicAgentInvocation::AgentMethodInvocation(method) => {
                let invocation_context = decode_public_span_data(&method.invocation_context, 0);
                Invocation::AgentMethod(
                    golem_api_grpc::proto::golem::worker::PublicAgentMethodInvocation {
                        idempotency_key: Some(method.idempotency_key.into()),
                        method_name: method.method_name,
                        function_input: Some(method.function_input.try_into()?),
                        trace_id: method.trace_id.to_string(),
                        trace_states: method.trace_states,
                        invocation_context,
                    },
                )
            }
            PublicAgentInvocation::ExternalTool(tool) => {
                let invocation_context = decode_public_span_data(&tool.invocation_context, 0);
                Invocation::ExternalTool(
                    golem_api_grpc::proto::golem::worker::PublicExternalToolInvocation {
                        idempotency_key: Some(tool.idempotency_key.into()),
                        tool_name: tool.tool_name,
                        command_path: tool.command_path,
                        input: Some(tool.input.try_into()?),
                        trace_id: tool.trace_id.to_string(),
                        trace_states: tool.trace_states,
                        invocation_context,
                    },
                )
            }
            PublicAgentInvocation::SaveSnapshot(_) => {
                Invocation::SaveSnapshot(golem_schema::proto::golem::common::Empty {})
            }
            PublicAgentInvocation::LoadSnapshot(load) => {
                let snapshot_data = match load.snapshot {
                    PublicSnapshotData::Raw(raw) => {
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Raw(
                                    golem_api_grpc::proto::golem::worker::RawSnapshotData {
                                        data: raw.data,
                                        mime_type: raw.mime_type,
                                    },
                                ),
                            ),
                        }
                    }
                    PublicSnapshotData::Json(json) => {
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Json(
                                    golem_api_grpc::proto::golem::worker::JsonSnapshotData {
                                        data: serde_json::to_string(&json.data)
                                            .map_err(|e| e.to_string())?,
                                    },
                                ),
                            ),
                        }
                    }
                    PublicSnapshotData::Multipart(multipart) => {
                        let parts = multipart.parts.into_iter().map(|p| {
                            let data = match p.data {
                                MultipartPartData::Json(json) => {
                                    golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Json(
                                        golem_api_grpc::proto::golem::worker::JsonSnapshotData {
                                            data: serde_json::to_string(&json.data).unwrap_or_default(),
                                        },
                                    )
                                }
                                MultipartPartData::Raw(raw) => {
                                    golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Raw(
                                        golem_api_grpc::proto::golem::worker::RawSnapshotData {
                                            data: raw.data,
                                            mime_type: raw.mime_type,
                                        },
                                    )
                                }
                            };
                            golem_api_grpc::proto::golem::worker::MultipartSnapshotPart {
                                name: p.name,
                                content_type: p.content_type,
                                data: Some(golem_api_grpc::proto::golem::worker::MultipartPartData {
                                    data: Some(data),
                                }),
                            }
                        }).collect();
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Multipart(
                                    golem_api_grpc::proto::golem::worker::MultipartSnapshotData {
                                        mime_type: multipart.mime_type,
                                        parts,
                                    },
                                ),
                            ),
                        }
                    }
                };
                Invocation::LoadSnapshot(
                    golem_api_grpc::proto::golem::worker::LoadSnapshotInvocationParameters {
                        snapshot: Some(snapshot_data),
                    },
                )
            }
            PublicAgentInvocation::ProcessOplogEntries(process) => Invocation::ProcessOplogEntries(
                golem_api_grpc::proto::golem::worker::ProcessOplogEntriesInvocationParameters {
                    idempotency_key: Some(process.idempotency_key.into()),
                },
            ),
            PublicAgentInvocation::ManualUpdate(manual) => Invocation::ManualUpdate(
                golem_api_grpc::proto::golem::worker::ManualUpdateInvocationParameters {
                    target_revision: manual.target_revision.into(),
                },
            ),
        };
        Ok(
            golem_api_grpc::proto::golem::worker::PublicAgentInvocation {
                invocation: Some(invocation),
            },
        )
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicExternalToolResult>
    for PublicExternalToolResult
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicExternalToolResult,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_external_tool_result::Result;
        match value.result.ok_or("Missing external tool result")? {
            Result::Success(success) => Ok(Self::Success(SerializableToolInvocationResult {
                result: success
                    .result
                    .map(TryInto::try_into)
                    .transpose()?
                    .map(Box::new),
            })),
            Result::Error(error) => Ok(Self::Failure(tool_rpc_error_from_proto(error)?)),
        }
    }
}

impl TryFrom<PublicExternalToolResult>
    for golem_api_grpc::proto::golem::worker::PublicExternalToolResult
{
    type Error = String;

    fn try_from(value: PublicExternalToolResult) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_external_tool_result::Result;
        let result = match value {
            PublicExternalToolResult::Success(result) => Result::Success(
                golem_api_grpc::proto::golem::worker::PublicToolInvocationResult {
                    result: result.result.map(|value| (*value).try_into()).transpose()?,
                },
            ),
            PublicExternalToolResult::Failure(error) => {
                Result::Error(tool_rpc_error_to_proto(error)?)
            }
        };
        Ok(Self {
            result: Some(result),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicAgentInvocationResult>
    for PublicAgentInvocationResult
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicAgentInvocationResult,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_agent_invocation_result::Result as ProtoResult;
        match value.result.ok_or("Missing result field")? {
            ProtoResult::AgentInitializationOutput(typed) => {
                let output = typed.try_into()?;
                Ok(PublicAgentInvocationResult::AgentInitialization(
                    AgentInvocationOutputParameters { output },
                ))
            }
            ProtoResult::AgentMethodOutput(typed) => {
                let output = typed.try_into()?;
                Ok(PublicAgentInvocationResult::AgentMethod(
                    AgentInvocationOutputParameters { output },
                ))
            }
            ProtoResult::ExternalTool(tool) => Ok(PublicAgentInvocationResult::ExternalTool(
                ExternalToolResultParameters {
                    result: tool.try_into()?,
                },
            )),
            ProtoResult::ManualUpdate(_) => Ok(PublicAgentInvocationResult::ManualUpdate(Empty {})),
            ProtoResult::LoadSnapshot(opt_err) => Ok(PublicAgentInvocationResult::LoadSnapshot(
                FallibleResultParameters {
                    error: opt_err.error,
                },
            )),
            ProtoResult::SaveSnapshot(snapshot) => {
                let data = match snapshot.data.ok_or("Missing data field")? {
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Raw(raw) => {
                        PublicSnapshotData::Raw(RawSnapshotData {
                            data: raw.data,
                            mime_type: raw.mime_type,
                        })
                    }
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Json(json) => {
                        PublicSnapshotData::Json(JsonSnapshotData {
                            data: serde_json::from_str(&json.data).map_err(|e| e.to_string())?,
                        })
                    }
                    golem_api_grpc::proto::golem::worker::snapshot_data::Data::Multipart(
                        multipart,
                    ) => PublicSnapshotData::Multipart(MultipartSnapshotData {
                        mime_type: multipart.mime_type,
                        parts: multipart.parts.into_iter().map(|p| {
                            let data = match p.data.and_then(|d| d.data) {
                                Some(golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Json(json)) => {
                                    MultipartPartData::Json(JsonSnapshotData {
                                        data: serde_json::from_str(&json.data).unwrap_or_default(),
                                    })
                                }
                                Some(golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Raw(raw)) => {
                                    MultipartPartData::Raw(RawSnapshotData {
                                        data: raw.data,
                                        mime_type: raw.mime_type,
                                    })
                                }
                                None => MultipartPartData::Raw(RawSnapshotData {
                                    data: vec![],
                                    mime_type: String::new(),
                                }),
                            };
                            MultipartSnapshotPart {
                                name: p.name,
                                content_type: p.content_type,
                                data,
                            }
                        }).collect(),
                    }),
                };
                Ok(PublicAgentInvocationResult::SaveSnapshot(
                    SaveSnapshotResultParameters { snapshot: data },
                ))
            }
            ProtoResult::ProcessOplogEntries(result) => {
                Ok(PublicAgentInvocationResult::ProcessOplogEntries(
                    ProcessOplogEntriesResultParameters {
                        error: result.error,
                    },
                ))
            }
        }
    }
}

impl TryFrom<PublicAgentInvocationResult>
    for golem_api_grpc::proto::golem::worker::PublicAgentInvocationResult
{
    type Error = String;

    fn try_from(value: PublicAgentInvocationResult) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_agent_invocation_result::Result as ProtoResult;
        let result = match value {
            PublicAgentInvocationResult::AgentInitialization(output) => {
                ProtoResult::AgentInitializationOutput(output.output.try_into()?)
            }
            PublicAgentInvocationResult::AgentMethod(output) => {
                ProtoResult::AgentMethodOutput(output.output.try_into()?)
            }
            PublicAgentInvocationResult::ExternalTool(tool) => {
                ProtoResult::ExternalTool(tool.result.try_into()?)
            }
            PublicAgentInvocationResult::ManualUpdate(_) => {
                ProtoResult::ManualUpdate(golem_schema::proto::golem::common::Empty {})
            }
            PublicAgentInvocationResult::LoadSnapshot(fallible) => {
                ProtoResult::LoadSnapshot(golem_api_grpc::proto::golem::worker::OptionalError {
                    error: fallible.error,
                })
            }
            PublicAgentInvocationResult::SaveSnapshot(save) => {
                let snapshot_data = match save.snapshot {
                    PublicSnapshotData::Raw(raw) => {
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Raw(
                                    golem_api_grpc::proto::golem::worker::RawSnapshotData {
                                        data: raw.data,
                                        mime_type: raw.mime_type,
                                    },
                                ),
                            ),
                        }
                    }
                    PublicSnapshotData::Json(json) => {
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Json(
                                    golem_api_grpc::proto::golem::worker::JsonSnapshotData {
                                        data: serde_json::to_string(&json.data)
                                            .map_err(|e| e.to_string())?,
                                    },
                                ),
                            ),
                        }
                    }
                    PublicSnapshotData::Multipart(multipart) => {
                        let parts = multipart.parts.into_iter().map(|p| {
                            let data = match p.data {
                                MultipartPartData::Json(json) => {
                                    golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Json(
                                        golem_api_grpc::proto::golem::worker::JsonSnapshotData {
                                            data: serde_json::to_string(&json.data).unwrap_or_default(),
                                        },
                                    )
                                }
                                MultipartPartData::Raw(raw) => {
                                    golem_api_grpc::proto::golem::worker::multipart_part_data::Data::Raw(
                                        golem_api_grpc::proto::golem::worker::RawSnapshotData {
                                            data: raw.data,
                                            mime_type: raw.mime_type,
                                        },
                                    )
                                }
                            };
                            golem_api_grpc::proto::golem::worker::MultipartSnapshotPart {
                                name: p.name,
                                content_type: p.content_type,
                                data: Some(golem_api_grpc::proto::golem::worker::MultipartPartData {
                                    data: Some(data),
                                }),
                            }
                        }).collect();
                        golem_api_grpc::proto::golem::worker::SnapshotData {
                            data: Some(
                                golem_api_grpc::proto::golem::worker::snapshot_data::Data::Multipart(
                                    golem_api_grpc::proto::golem::worker::MultipartSnapshotData {
                                        mime_type: multipart.mime_type,
                                        parts,
                                    },
                                ),
                            ),
                        }
                    }
                };
                ProtoResult::SaveSnapshot(snapshot_data)
            }
            PublicAgentInvocationResult::ProcessOplogEntries(result) => {
                ProtoResult::ProcessOplogEntries(
                    golem_api_grpc::proto::golem::worker::ProcessOplogEntriesResult {
                        error: result.error,
                    },
                )
            }
        };
        Ok(
            golem_api_grpc::proto::golem::worker::PublicAgentInvocationResult {
                result: Some(result),
            },
        )
    }
}

fn tool_rpc_error_from_proto(
    value: golem_api_grpc::proto::golem::worker::PublicToolRpcError,
) -> Result<SerializableToolRpcError, String> {
    use golem_api_grpc::proto::golem::worker::public_tool_error::Error as ToolError;
    use golem_api_grpc::proto::golem::worker::public_tool_rpc_error::Error as RpcError;
    Ok(match value.error.ok_or("Missing tool RPC error")? {
        RpcError::ProtocolError(value) => SerializableToolRpcError::ProtocolError(value),
        RpcError::Denied(value) => SerializableToolRpcError::Denied(value),
        RpcError::NotFound(value) => SerializableToolRpcError::NotFound(value),
        RpcError::RemoteInternalError(value) => {
            SerializableToolRpcError::RemoteInternalError(value)
        }
        RpcError::Cancelled(_) => SerializableToolRpcError::Cancelled,
        RpcError::ResourceExhausted(value) => SerializableToolRpcError::ResourceExhausted(value),
        RpcError::RemoteToolError(value) => SerializableToolRpcError::RemoteToolError(Box::new(
            match value.error.ok_or("Missing remote tool error")? {
                ToolError::InvalidToolName(value) => SerializableToolError::InvalidToolName(value),
                ToolError::InvalidCommandPath(value) => {
                    SerializableToolError::InvalidCommandPath(value.values)
                }
                ToolError::InvalidInput(value) => SerializableToolError::InvalidInput(value),
                ToolError::ConstraintViolation(value) => {
                    SerializableToolError::ConstraintViolation(value)
                }
                ToolError::InvalidResult(value) => SerializableToolError::InvalidResult(value),
                ToolError::CustomError(value) => {
                    SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                        name: value.name,
                        payload: value
                            .payload
                            .ok_or("Missing custom tool error payload")?
                            .try_into()?,
                    }))
                }
            },
        )),
    })
}

fn tool_rpc_error_to_proto(
    value: SerializableToolRpcError,
) -> Result<golem_api_grpc::proto::golem::worker::PublicToolRpcError, String> {
    use golem_api_grpc::proto::golem::worker::public_tool_error::Error as ToolError;
    use golem_api_grpc::proto::golem::worker::public_tool_rpc_error::Error as RpcError;
    let error = match value {
        SerializableToolRpcError::ProtocolError(value) => RpcError::ProtocolError(value),
        SerializableToolRpcError::Denied(value) => RpcError::Denied(value),
        SerializableToolRpcError::NotFound(value) => RpcError::NotFound(value),
        SerializableToolRpcError::RemoteInternalError(value) => {
            RpcError::RemoteInternalError(value)
        }
        SerializableToolRpcError::Cancelled => {
            RpcError::Cancelled(golem_schema::proto::golem::common::Empty {})
        }
        SerializableToolRpcError::ResourceExhausted(value) => RpcError::ResourceExhausted(value),
        SerializableToolRpcError::RemoteToolError(value) => {
            let error = match *value {
                SerializableToolError::InvalidToolName(value) => ToolError::InvalidToolName(value),
                SerializableToolError::InvalidCommandPath(values) => ToolError::InvalidCommandPath(
                    golem_api_grpc::proto::golem::worker::StringList { values },
                ),
                SerializableToolError::InvalidInput(value) => ToolError::InvalidInput(value),
                SerializableToolError::ConstraintViolation(value) => {
                    ToolError::ConstraintViolation(value)
                }
                SerializableToolError::InvalidResult(value) => ToolError::InvalidResult(value),
                SerializableToolError::CustomError(value) => ToolError::CustomError(
                    golem_api_grpc::proto::golem::worker::PublicCustomToolError {
                        name: value.name,
                        payload: Some(value.payload.try_into()?),
                    },
                ),
            };
            RpcError::RemoteToolError(golem_api_grpc::proto::golem::worker::PublicToolError {
                error: Some(error),
            })
        }
    };
    Ok(golem_api_grpc::proto::golem::worker::PublicToolRpcError { error: Some(error) })
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicEntityInvocationContext>
    for PublicEntityInvocationContext
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicEntityInvocationContext,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            invocation: value
                .invocation
                .ok_or("Missing entity invocation")?
                .try_into()?,
            ancestors: value
                .ancestors
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<PublicEntityInvocationContext>
    for golem_api_grpc::proto::golem::worker::PublicEntityInvocationContext
{
    fn from(value: PublicEntityInvocationContext) -> Self {
        Self {
            invocation: Some(value.invocation.into()),
            ancestors: value.ancestors.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicEntityInvocation>
    for PublicEntityInvocation
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicEntityInvocation,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::PublicEntityCallMode as ProtoCallMode;

        let call_mode = match ProtoCallMode::try_from(value.call_mode)
            .map_err(|_| format!("Invalid PublicEntityCallMode: {}", value.call_mode))?
        {
            ProtoCallMode::Synchronous => PublicEntityCallMode::Synchronous,
            ProtoCallMode::Asynchronous => PublicEntityCallMode::Asynchronous,
            ProtoCallMode::FireAndForget => PublicEntityCallMode::FireAndForget,
            ProtoCallMode::Unspecified => {
                return Err("Unspecified PublicEntityCallMode".to_string());
            }
        };
        let start_index = value.start_index.ok_or("Missing entity start index")?;
        if start_index == 0 {
            return Err("Invalid entity start index: 0".to_string());
        }
        Ok(Self {
            entity: value
                .entity
                .ok_or("Missing public agent entity")?
                .try_into()?,
            start_index: OplogIndex::from_u64(start_index),
            call_mode,
            operation: value.operation.map(TryInto::try_into).transpose()?,
        })
    }
}

impl From<PublicEntityInvocation> for golem_api_grpc::proto::golem::worker::PublicEntityInvocation {
    fn from(value: PublicEntityInvocation) -> Self {
        use golem_api_grpc::proto::golem::worker::PublicEntityCallMode as ProtoCallMode;

        let call_mode = match value.call_mode {
            PublicEntityCallMode::Synchronous => ProtoCallMode::Synchronous,
            PublicEntityCallMode::Asynchronous => ProtoCallMode::Asynchronous,
            PublicEntityCallMode::FireAndForget => ProtoCallMode::FireAndForget,
        };
        Self {
            entity: Some(value.entity.into()),
            start_index: Some(value.start_index.into()),
            call_mode: call_mode as i32,
            operation: value.operation.map(Into::into),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicAgentEntity> for PublicAgentEntity {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicAgentEntity,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::PublicAgentEntityKind as ProtoKind;

        let kind = match ProtoKind::try_from(value.kind)
            .map_err(|_| format!("Invalid PublicAgentEntityKind: {}", value.kind))?
        {
            ProtoKind::Tool => PublicAgentEntityKind::Tool,
            ProtoKind::ToolMiddleware => PublicAgentEntityKind::ToolMiddleware,
            ProtoKind::Unspecified => return Err("Unspecified PublicAgentEntityKind".to_string()),
        };
        Ok(Self {
            kind,
            name: value.name,
        })
    }
}

impl From<PublicAgentEntity> for golem_api_grpc::proto::golem::worker::PublicAgentEntity {
    fn from(value: PublicAgentEntity) -> Self {
        use golem_api_grpc::proto::golem::worker::PublicAgentEntityKind as ProtoKind;

        let kind = match value.kind {
            PublicAgentEntityKind::Tool => ProtoKind::Tool,
            PublicAgentEntityKind::ToolMiddleware => ProtoKind::ToolMiddleware,
        };
        Self {
            kind: kind as i32,
            name: value.name,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PublicEntityInvocationOperation>
    for PublicEntityInvocationOperation
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PublicEntityInvocationOperation,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::public_entity_invocation_operation::Operation;

        match value
            .operation
            .ok_or("Missing entity invocation operation")?
        {
            Operation::Tool(tool) => Ok(Self::Tool(PublicToolInvocationOperation {
                command_path: tool.command_path,
                has_stdin: tool.has_stdin,
                has_stdout: tool.has_stdout,
                declares_stdout: tool.declares_stdout,
                has_stderr: tool.has_stderr,
                declares_stderr: tool.declares_stderr,
            })),
        }
    }
}

impl From<PublicEntityInvocationOperation>
    for golem_api_grpc::proto::golem::worker::PublicEntityInvocationOperation
{
    fn from(value: PublicEntityInvocationOperation) -> Self {
        use golem_api_grpc::proto::golem::worker::public_entity_invocation_operation::Operation;

        let operation = match value {
            PublicEntityInvocationOperation::Tool(tool) => Operation::Tool(
                golem_api_grpc::proto::golem::worker::PublicToolInvocationOperation {
                    command_path: tool.command_path,
                    has_stdin: tool.has_stdin,
                    has_stdout: tool.has_stdout,
                    declares_stdout: tool.declares_stdout,
                    has_stderr: tool.has_stderr,
                    declares_stderr: tool.declares_stderr,
                },
            ),
        };
        Self {
            operation: Some(operation),
        }
    }
}

pub(super) fn public_agent_invocation_result_to_raw(
    result: PublicAgentInvocationResult,
) -> Result<AgentInvocationResult, String> {
    match result {
        PublicAgentInvocationResult::AgentInitialization(_) => {
            Ok(AgentInvocationResult::AgentInitialization)
        }
        PublicAgentInvocationResult::AgentMethod(params) => {
            Ok(AgentInvocationResult::AgentMethod {
                output: params.output.into_parts().1,
            })
        }
        PublicAgentInvocationResult::ExternalTool(params) => {
            Ok(AgentInvocationResult::ExternalTool {
                result: match params.result {
                    PublicExternalToolResult::Success(result) => Ok(result),
                    PublicExternalToolResult::Failure(error) => Err(error),
                },
            })
        }
        PublicAgentInvocationResult::ManualUpdate(_) => Ok(AgentInvocationResult::ManualUpdate),
        PublicAgentInvocationResult::LoadSnapshot(params) => {
            Ok(AgentInvocationResult::LoadSnapshot {
                error: params.error,
            })
        }
        PublicAgentInvocationResult::SaveSnapshot(params) => {
            let snapshot = match params.snapshot {
                PublicSnapshotData::Raw(raw) => raw,
                PublicSnapshotData::Json(json) => RawSnapshotData {
                    data: serde_json::to_vec(&json.data).map_err(|e| e.to_string())?,
                    mime_type: "application/json".to_string(),
                },
                PublicSnapshotData::Multipart(multipart) => {
                    use crate::base_model::oplog::multipart::extract_boundary;
                    use crate::model::oplog::MultipartPartData;

                    let boundary = extract_boundary(&multipart.mime_type)
                        .unwrap_or("boundary")
                        .to_string();
                    let mut output = Vec::new();
                    for part in &multipart.parts {
                        output.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                        output.extend_from_slice(
                            format!("Content-Type: {}\r\n", part.content_type).as_bytes(),
                        );
                        output.extend_from_slice(
                            format!(
                                "Content-Disposition: attachment; name=\"{}\"\r\n",
                                part.name
                            )
                            .as_bytes(),
                        );
                        output.extend_from_slice(b"\r\n");
                        match &part.data {
                            MultipartPartData::Json(json) => {
                                output.extend_from_slice(
                                    serde_json::to_vec(&json.data)
                                        .unwrap_or_default()
                                        .as_slice(),
                                );
                            }
                            MultipartPartData::Raw(raw) => {
                                output.extend_from_slice(&raw.data);
                            }
                        }
                        output.extend_from_slice(b"\r\n");
                    }
                    output.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
                    RawSnapshotData {
                        data: output,
                        mime_type: multipart.mime_type,
                    }
                }
            };
            Ok(AgentInvocationResult::SaveSnapshot { snapshot })
        }
        PublicAgentInvocationResult::ProcessOplogEntries(params) => {
            Ok(AgentInvocationResult::ProcessOplogEntries {
                error: params.error,
            })
        }
    }
}
