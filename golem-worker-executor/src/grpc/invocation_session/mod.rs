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

use super::{WorkerExecutorImpl, extract_owned_agent_id};
use crate::durable_host::durable_session::{
    StreamSession, durable_stream_mapping_from_proto, durable_stream_mapping_to_proto,
};
use crate::durable_host::durable_stream::{ProducerRegistrationRequest, SessionError};
use crate::durable_host::stream_session::{
    decode_recursive_stream_value, decode_recursive_stream_value_with_schema,
    encode_recursive_stream_value_with_schema,
};
use crate::grpc::invocation::{CanStartWorker, from_proto_invocation_context};
use crate::services::oplog::OplogError;
use crate::services::{HasAll, HasComponentService, HasSchedulerService, UsesAllDeps};
use crate::worker::invocation::validate_agent_method_invocation;
use crate::worker::{DurableStreamingInvocationRequest, Worker};
use crate::workerctx::WorkerCtx;
use chrono::{DateTime, Utc};
use futures::Stream;
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError;
use golem_api_grpc::proto::golem::worker::{
    InputStreamAck, InvocationAccepted, InvocationFailure, InvocationFailureKind,
    InvocationRejected, InvocationRejectionReason, InvocationRequest, InvocationResponse,
    InvocationSessionCompletion, InvocationSessionResult, InvocationStart, ResumeAttach,
    ResumeOperation, invocation_request, invocation_response, invocation_session_completion,
    invocation_session_result,
};
use golem_common::base_model::durable_stream::{
    MAX_DURABLE_STREAM_ITEM_SIZE, MAX_NEW_STREAM_HANDLES_PER_VALUE, ResumeAttemptDescriptor,
    StreamCancelReason, StreamCancelRole, StreamItemsPayload, StreamRegistrationInvocation,
    StreamResumeCursor, StreamResumeOperation,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentMode, InvocationFreshnessDisposition, ParsedAgentId, Principal,
};
use golem_common::model::card::ScopeCard;
use golem_common::model::component::ComponentRevision;
use golem_common::model::durable_stream::{
    AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, PersistedInvocationTarget,
    PersistedStreamInvocationDescriptor, SessionStreamRole, StartAttemptDescriptor,
    StreamInvocationId, StreamRegistrationCoordinate, StreamRootKind, StreamSessionMapping,
    StreamSourceKind, StreamValuePathStep, ToolByteStreamRole,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::tool::{ToolInvocationInput, ToolInvocationOutput};
use golem_common::model::{
    AgentId, AgentInvocation, AgentInvocationOutput, AgentInvocationResult, IdempotencyKey,
    InvocationStatus, ScheduledAction,
};
use golem_common::schema::SchemaValue;
use golem_common::schema::agent::FieldSource;
use golem_common::schema::conversion::{FromSchema, IntoTypedSchemaValue};
use golem_schema::schema::{
    NamedFieldType, SchemaGraph, SchemaType, SchemaValueStream, schema_fingerprint_v1,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::grpc::proto_agent_id_string;
use prost::Message;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tonic::{Request, Response, Status};
use tracing::Instrument;

mod external_tool;

pub(super) type InvocationSessionStream =
    Pin<Box<dyn Stream<Item = Result<InvocationResponse, Status>> + Send + 'static>>;

type ResponseLease = Arc<std::sync::Mutex<Option<Arc<crate::worker::EphemeralResponseLease>>>>;

pub(super) async fn invoke_agent_session<
    Ctx: WorkerCtx,
    Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static,
>(
    executor: &WorkerExecutorImpl<Ctx, Svcs>,
    request: Request<tonic::Streaming<InvocationRequest>>,
) -> Result<Response<InvocationSessionStream>, Status> {
    let mut inbound = request.into_inner();
    // A decode failure has no invocation identity and must remain a transport error.
    let first = inbound.message().await?;
    let (responses, receiver) = mpsc::channel(32);
    let response_lease = ResponseLease::default();
    let handler_lease = response_lease.clone();
    let executor = (*executor).clone();
    let span = tracing::info_span!(
        "invoke_agent_session",
        agent_id = tracing::field::Empty,
        idempotency_key = tracing::field::Empty
    );
    tokio::spawn(
        async move {
            let scope = crate::worker::tasks::TaskScope::default();
            scope
                .run(Box::pin(executor.run_agent_session(
                    inbound,
                    first,
                    responses,
                    handler_lease,
                    &scope,
                )))
                .await;
        }
        .instrument(span),
    );
    Ok(Response::new(retained_response_stream(
        receiver,
        response_lease,
    )))
}

fn retained_response_stream(
    receiver: mpsc::Receiver<InvocationResponse>,
    response_lease: ResponseLease,
) -> InvocationSessionStream {
    Box::pin(futures::stream::unfold(
        (receiver, response_lease),
        |(mut receiver, lease)| async move {
            receiver
                .recv()
                .await
                .map(|response| (Ok(response), (receiver, lease)))
        },
    ))
}

struct AcceptedInvocation {
    component_revision: Option<ComponentRevision>,
    durable_streams: Option<StreamSession>,
    prepared: Option<golem_common::model::durable_stream::StreamSessionPreparedRecord>,
    durable_replayed: bool,
    joined_origin_observer: bool,
}

struct TransportStreamId(u64);

mod admission;
#[cfg(test)]
use admission::decode_invocation_freshness_disposition;
use admission::{AcceptanceRace, publish_acceptance, race_invocation_acceptance};

mod request;
#[cfg(test)]
use request::effective_session_identity;
pub(crate) use request::{build_durable_streaming_request, decode_invocation_input};
use request::{
    build_resume_attempt, request_agent_id, request_idempotency_key,
    require_expected_callee_fingerprint, resumed_input_schema, route_durable_request,
};

mod response;
use response::{
    persisted_invocation_result, persisted_result_and_finished, pre_acceptance_rejection_reason,
    rejection_worker_error, send_attachment_revocation, send_protocol_failure, send_rejection,
    send_rejection_with_worker_error, send_resumed_finished, send_unvalidated_rejection,
    send_worker_failure,
};

mod attachment;
use attachment::{
    detach_durable_attachment, end_attachment_on_failure, ends_only_the_attachment,
    is_attachment_termination, until_response_closed,
};

#[cfg(test)]
mod freshness_tests;

#[cfg(test)]
mod tests;

mod resume;
mod start;
