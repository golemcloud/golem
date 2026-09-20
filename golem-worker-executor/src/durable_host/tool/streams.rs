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

use super::{
    SerializableToolError, SerializableToolResultValue, SerializableToolRpcError,
    SerializableToolStructuredResult, ToolInvokeResponse, encode_tool_operation_terminal,
};
use crate::durable_host::durable_session::{StreamSession, strip_typed_streams};
use crate::durable_host::schema_value_stream::contains_stream;
use crate::durable_host::tail_work::TailActivity;
use crate::services::HasOplog;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use golem_common::model::IdempotencyKey;
use golem_common::model::component::ComponentRevision;
use golem_common::model::durable_stream::{
    SessionStreamRole, StreamRegistrationInvocation, StreamSessionKey,
};
use golem_common::model::entity::EntityInvocationScope;
use golem_common::model::oplog::payload::HostResponseEntityInvocation;
use golem_common::schema::TypedSchemaValue;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use tokio::sync::oneshot;
use wasmtime::component::{Accessor, AccessorTask};

pub(super) struct MaterializeResponse {
    pub streams: StreamSession,
    pub revision: ComponentRevision,
    pub response: ToolInvokeResponse,
    pub completed: oneshot::Sender<Result<HostResponseEntityInvocation, WorkerExecutorError>>,
    pub activity: TailActivity,
}

impl<Ctx: WorkerCtx> AccessorTask<Ctx> for MaterializeResponse {
    async fn run(self, _accessor: &Accessor<Ctx>) -> wasmtime::Result<()> {
        let Self {
            streams,
            revision,
            response,
            completed,
            activity: _activity,
        } = self;
        let result = async {
            let response = materialize_response(&streams, revision, response).await?;
            let response = response.map(|result| SerializableToolStructuredResult {
                result: result
                    .result
                    .as_deref()
                    .map(SerializableToolResultValue::from_typed)
                    .transpose()
                    .expect("a materialized tool result is serializable"),
            });
            encode_tool_operation_terminal(response).await
        }
        .await;
        let _ = completed.send(result);
        Ok(())
    }
}

pub(super) async fn session<Ctx: WorkerCtx>(
    worker: &Arc<Worker<Ctx>>,
    scope: &EntityInvocationScope,
) -> Result<StreamSession, WorkerExecutorError> {
    let owner = worker.get_initial_worker_metadata();
    let key = StreamSessionKey {
        callee_environment_id: owner.environment_id,
        callee: owner.agent_id,
        callee_fingerprint: owner.fingerprint,
        idempotency_key: scope.stream_session_idempotency_key().clone(),
    };
    let mut consumer = key.clone();
    consumer.idempotency_key =
        IdempotencyKey::derived_from_bytes(&key.idempotency_key, b"golem:tool-result-consumer:v1");
    Ok(StreamSession::new(
        worker.durable_stream_producer().await?,
        worker.oplog(),
        StreamRegistrationInvocation::Local(key.idempotency_key),
        [],
    )
    .with_consumer_invocation(consumer)
    .with_consumer_journal(worker.durable_stream_consumer_journal())
    .with_entity_parent_start_index(Some(scope.invocation_id().start_index())))
}

pub(super) async fn materialize_input(
    streams: &StreamSession,
    revision: ComponentRevision,
    input: &TypedSchemaValue,
) -> Result<TypedSchemaValue, WorkerExecutorError> {
    if !contains_stream(input.value()) {
        return Ok(input.clone());
    }
    let materialized = streams
        .materialize_agent_input(input.value(), input.graph(), &input.graph().root, revision)
        .await
        .map_err(WorkerExecutorError::runtime)?;
    let value = streams
        .decode_initial(
            materialized.value,
            &materialized.mappings,
            SessionStreamRole::Input,
        )
        .await
        .map_err(WorkerExecutorError::runtime)?;
    Ok(TypedSchemaValue::new(input.graph().clone(), value))
}

fn payload(response: &mut ToolInvokeResponse) -> Option<&mut TypedSchemaValue> {
    match response {
        Ok(result) => result.result.as_deref_mut(),
        Err(SerializableToolRpcError::RemoteToolError(error)) => match error.as_mut() {
            SerializableToolError::CustomError(error) => Some(&mut error.payload),
            _ => None,
        },
        _ => None,
    }
}

fn strip_response(mut response: ToolInvokeResponse) -> ToolInvokeResponse {
    if let Some(value) = payload(&mut response) {
        *value = strip_typed_streams(value);
    }
    response
}

pub(super) async fn restore_response(
    streams: &StreamSession,
    mut response: ToolInvokeResponse,
) -> Result<ToolInvokeResponse, WorkerExecutorError> {
    if let Some(value) = payload(&mut response)
        && let Some(materialized) = streams
            .replay_remote_result()
            .await
            .map_err(WorkerExecutorError::runtime)?
    {
        *value = TypedSchemaValue::new(value.graph().clone(), materialized);
    }
    Ok(response)
}

pub(super) async fn materialize_response(
    streams: &StreamSession,
    revision: ComponentRevision,
    mut response: ToolInvokeResponse,
) -> Result<ToolInvokeResponse, WorkerExecutorError> {
    let Some(value) = payload(&mut response) else {
        return Ok(response);
    };
    if !contains_stream(value.value()) {
        return Ok(response);
    }
    let graph = value.graph().clone();
    let value = value.value().clone();
    let stripped = strip_response(response);
    streams
        .materialize_result(value, &graph, &graph.root, revision)
        .await
        .map_err(WorkerExecutorError::runtime)?;
    Ok(stripped)
}
