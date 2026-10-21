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

use crate::durable_host::DurableWorkerCtx;
use crate::preview2::golem::agent::host::InvocationMetadata;
use crate::services::rpc::RpcError as InternalRpcError;
use crate::workerctx::WorkerCtx;
use golem_common::model::IdempotencyKey;
use golem_common::model::OwnedAgentId;
use golem_common::model::invocation_context::AttributeValue;
use golem_common::model::invocation_context::InvocationContextSpan;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::AttributeMap;
use golem_common::model::oplog::SpanFinished;
use golem_common::model::oplog::SpanKind;
use golem_common::model::oplog::SpanOutcome;
use golem_common::model::oplog::SpanStarted;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashMap;
use std::sync::Arc;

pub(super) fn invocation_metadata(
    remote_agent_id: &OwnedAgentId,
    idempotency_key: &IdempotencyKey,
) -> InvocationMetadata {
    InvocationMetadata {
        agent_id: remote_agent_id.agent_id.agent_id.clone(),
        idempotency_key: idempotency_key.value.clone(),
    }
}

pub(super) fn rpc_span_finished<T>(
    span_id: &SpanId,
    result: &Result<T, InternalRpcError>,
) -> SpanFinished {
    let outcome = match result {
        Ok(_) => SpanOutcome::Completed,
        Err(InternalRpcError::Denied { .. }) => SpanOutcome::Denied,
        Err(_) => SpanOutcome::Failed,
    };
    SpanFinished {
        span_id: span_id.clone(),
        finished_at: golem_common::model::Timestamp::now_utc(),
        outcome,
    }
}

pub(super) fn create_embedded_invocation_span<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    connection_span_id: &SpanId,
    function_name: &str,
    idempotency_key: &IdempotencyKey,
) -> Result<(Arc<InvocationContextSpan>, SpanStarted), WorkerExecutorError> {
    let connection_origin = ctx
        .state
        .invocation_context
        .span_origin(connection_span_id)
        .ok_or_else(|| WorkerExecutorError::runtime("RPC connection span has no trace origin"))?;
    let started = embedded_invocation_span_started(
        &ctx.state.invocation_context.trace_id,
        &ctx.state.invocation_context.trace_states,
        &ctx.state.current_span_id,
        connection_span_id,
        connection_origin,
        function_name,
        idempotency_key,
    );
    let span = install_embedded_span(ctx, &started, connection_span_id)?;
    Ok((span, started))
}

pub(super) fn embedded_invocation_span_started(
    trace_id: &golem_common::model::invocation_context::TraceId,
    trace_states: &[String],
    current_span_id: &SpanId,
    connection_span_id: &SpanId,
    (connection_trace_id, connection_trace_states): (
        golem_common::model::invocation_context::TraceId,
        Vec<String>,
    ),
    function_name: &str,
    idempotency_key: &IdempotencyKey,
) -> SpanStarted {
    let attributes = HashMap::from([
        (
            "name".to_string(),
            AttributeValue::String("rpc-invocation".to_string()),
        ),
        (
            "function_name".to_string(),
            AttributeValue::String(function_name.to_string()),
        ),
        (
            "idempotency_key".to_string(),
            AttributeValue::String(idempotency_key.to_string()),
        ),
    ]);
    let same_trace = connection_trace_id == *trace_id;
    let links = (!same_trace)
        .then(|| golem_common::model::oplog::SpanLink {
            trace_id: connection_trace_id,
            span_id: connection_span_id.clone(),
            trace_states: connection_trace_states,
        })
        .into_iter()
        .collect();
    SpanStarted {
        span_id: SpanId::generate(),
        trace_id: trace_id.clone(),
        trace_states: trace_states.to_vec(),
        parent_span_id: Some(if same_trace {
            connection_span_id.clone()
        } else {
            current_span_id.clone()
        }),
        links,
        started_at: golem_common::model::Timestamp::now_utc(),
        attributes: AttributeMap(attributes),
        kind: SpanKind::Client,
    }
}

pub(super) fn restore_embedded_span<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    started: &SpanStarted,
) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError> {
    let connection_span_id = started
        .links
        .first()
        .map(|link| &link.span_id)
        .or(started.parent_span_id.as_ref())
        .ok_or_else(|| WorkerExecutorError::runtime("RPC invocation span has no connection"))?;
    install_embedded_span(ctx, started, connection_span_id)
}

pub(super) fn install_embedded_span<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    started: &SpanStarted,
    connection_span_id: &SpanId,
) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError> {
    let parent = started
        .parent_span_id
        .as_ref()
        .map(|parent| ctx.state.invocation_context.get(parent))
        .transpose()
        .map_err(WorkerExecutorError::runtime)?;
    let mut builder = InvocationContextSpan::local()
        .with_span_id(started.span_id.clone())
        .with_start(started.started_at)
        .with_attributes(started.attributes.0.clone());
    if let Some(parent) = parent {
        builder = builder.with_parent(parent);
    }
    if !started.links.is_empty() {
        let connection = ctx
            .state
            .invocation_context
            .get(connection_span_id)
            .map_err(WorkerExecutorError::runtime)?;
        builder = builder.with_linked_context(connection);
    }
    let span = builder.build();
    ctx.state.invocation_context.add_span_with_origin(
        span.clone(),
        started.trace_id.clone(),
        started.trace_states.clone(),
    );
    Ok(span)
}
