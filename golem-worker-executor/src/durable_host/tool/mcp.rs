// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License").
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use crate::durable_host::concurrent::{
    CallReplayOutcome, DurableCallSession, LeaveIncompleteOnDrop,
};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
use crate::durable_host::{DurableWorkerCtx, InternalRetryResult};
use crate::native_tool::{NativeToolInvocation, NativeToolResult};
use crate::preview2::golem::tool::host::ByteStreamFailure;
use crate::services::environment_state::ToolDiscoveryError;
use crate::services::oplog::CommitLevel;
use crate::services::{HasMcpTransport, HasWorker};
use crate::worker::invocation::InvokeResult;
use crate::workerctx::WorkerCtx;
use golem_common::model::entity::{
    EntityActivationPolicy, InvocationExecutionMode, McpImportActivation,
};
use golem_common::model::mcp_import::McpImportCredential;
use golem_common::model::oplog::host_functions::{McpToolCall, McpToolPresence};
use golem_common::model::oplog::payload::types::{
    SerializableCustomToolError, SerializableToolError, SerializableToolResultValue,
    SerializableToolRpcError, SerializableToolStructuredResult,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestMcpToolCall, HostRequestMcpToolPresence,
    HostResponseMcpToolCall, HostResponseMcpToolPresence,
};
use golem_common::schema::graph::reachable_defs;
use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
use golem_mcp_import::tool::{CallError, ProjectedTool};
use golem_mcp_import::transport::{Client, TransportError};
use golem_service_base::clients::registry::RegistryServiceError;
use golem_service_base::error::worker_executor::{GolemSpecificWasmTrap, WorkerExecutorError};
use golem_service_base::model::auth::AuthCtx;
use serde::Deserialize;
use serde_json::Value;

pub(super) async fn invoke<Ctx: WorkerCtx>(
    ctx: &mut Ctx,
    invocation: NativeToolInvocation,
) -> Result<NativeToolResult, WorkerExecutorError> {
    let result = invoke_inner(ctx.durable_ctx_mut(), invocation).await;
    match result {
        Ok(result) => Ok(result),
        Err(error) => {
            // Preserve suspension, invocation quotas and call-owned retry points;
            // converting the error to a string here would lose the host trap.
            let trap = InvokeResult::from_error::<Ctx>(
                0,
                &error,
                ctx.get_current_retry_point().await,
                ctx.current_in_atomic_region(),
                ctx.current_atomic_region_had_side_effects(),
                ctx.agent_mode(),
            )
            .as_trap_type::<Ctx>()
            .expect("failed MCP host call must classify as a trap");
            let durable = ctx.durable_ctx();
            let completed = durable
                .entity_invocation_scope()
                .is_some_and(|scope| scope.mode() == InvocationExecutionMode::ReplayingCompleted);
            if !completed && let Some(operation) = durable.entity_tool_operation() {
                operation.select_trap(trap.clone()).await;
            }
            Err(trap
                .as_golem_error("")
                .unwrap_or_else(|| WorkerExecutorError::runtime("MCP tool invocation interrupted")))
        }
    }
}

async fn invoke_inner<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    invocation: NativeToolInvocation,
) -> anyhow::Result<NativeToolResult> {
    let scope = ctx
        .entity_invocation_scope()
        .ok_or_else(|| anyhow::anyhow!("missing MCP invocation scope"))?;
    let EntityActivationPolicy::Tool {
        binding,
        mcp_import: Some(activation),
        ..
    } = scope.activation().policy()
    else {
        anyhow::bail!("missing MCP activation projection");
    };
    let activation = activation.clone();
    let name = binding.tool_name.to_string();
    let digest = binding.metadata_digest.to_string();
    let tool = tokio::task::spawn_blocking({
        let activation = activation.clone();
        move || {
            let tool = ProjectedTool::from_json(&activation.projected_tool)?;
            anyhow::ensure!(
                tool.definition.name() == Some(name.as_str())
                    && tool.digest == digest
                    && tool.upstream_name == activation.source.upstream_tool_name,
                "MCP projection does not match its admitted binding"
            );
            Ok::<_, anyhow::Error>(tool)
        }
    })
    .await?
    .map_err(|error| ClassifiedHostError {
        kind: HostFailureKind::Permanent,
        message: error.to_string(),
    })?;

    let mut call =
        DurableCallSession::<McpToolCall, LeaveIncompleteOnDrop>::start_with_agent_authority(
            ctx,
            HostRequestMcpToolCall {
                input: invocation.input.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;
    // Consume the logical atomic-region position on live AND replay, exactly as RPC does.
    let key = ctx.derive_idempotency_key(call.begin_index()).to_string();
    let response = 'response: {
        if !call.is_live() {
            match call.replay(ctx).await? {
                CallReplayOutcome::Replayed(response) => break 'response response,
                CallReplayOutcome::Incomplete(live) => call = live,
            }
        }
        let auth = call.take_agent_auth_ctx();
        let mut unauthorized_generation = None;
        let result = loop {
            ctx.public_state
                .worker()
                .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                .await;
            let result = tokio::select! {
                biased;
                _ = async {
                    match &invocation.cancellation {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let response = call.complete(ctx, HostResponseMcpToolCall {
                        result: Err(SerializableToolRpcError::Cancelled),
                    }).await?;
                    ctx.public_state.worker().commit_oplog_and_update_state(CommitLevel::DurableOnly).await;
                    break 'response response;
                }
                result = live_call(ctx, &activation, &tool, &invocation.input, &auth, &key, &mut unauthorized_generation) => result,
            };
            let result = match result {
                Err(error)
                    if matches!(
                        error.downcast_ref::<RegistryServiceError>(),
                        Some(RegistryServiceError::LimitExceeded(_))
                    ) =>
                {
                    return Err(
                        call.trap(GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted)
                    );
                }
                Err(error)
                    if error.downcast_ref::<TransportError>().is_none()
                        && error.downcast_ref::<RegistryServiceError>().is_none()
                        && error.downcast_ref::<ToolDiscoveryError>().is_none() =>
                {
                    return Err(call.trap(error));
                }
                result => result,
            };
            match call
                .try_trigger_retry_or_loop(ctx, &result, classify_error)
                .await?
            {
                InternalRetryResult::RetryInternally => continue,
                InternalRetryResult::Persist => break result,
            }
        };
        let result = match result {
            Ok(value) => Ok(encode_response(Ok(value)).await?),
            Err(error) => match error.downcast::<TransportError>() {
                Ok(error) => Ok(encode_response(Err(error)).await?),
                Err(error) => Err(match error.downcast_ref::<RegistryServiceError>() {
                    Some(error) => registry_failure(error),
                    None => SerializableToolRpcError::RemoteInternalError(error.to_string()),
                }),
            },
        };
        let response = call
            .complete(ctx, HostResponseMcpToolCall { result })
            .await?;
        // This commit precedes all projection, stdout publication and feedback.
        ctx.public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
            .await;
        if let Some(generation) = unauthorized_generation {
            // Feedback never retries the tools/call whose response is already durable.
            let _ = ctx
                .state
                .environment_state_service
                .report_mcp_resource_unauthorized(&activation.source, &auth, generation)
                .await;
        }
        response
    };
    let result = match response.result {
        Ok(bytes) => {
            let response = tokio::task::spawn_blocking(move || decode_response(&bytes)).await??;
            let present = if matches!(&response, Err(TransportError::Remote { code: -32602, .. })) {
                tool_presence(ctx, &activation, invocation.cancellation.as_ref()).await?
            } else {
                Ok(None)
            };
            match present {
                Ok(present) => {
                    tokio::task::spawn_blocking(move || project_response(tool, response, present))
                        .await??
                }
                Err(cancelled) => Err(cancelled),
            }
        }
        Err(error) => Err(error),
    };
    let result = match result {
        Ok((value, bytes)) => {
            if let (Some(stdout), Some(bytes)) = (&invocation.stdout, bytes) {
                // Use the same bounded attachment writer as ordinary native tools.
                for chunk in bytes.chunks(16 * 1024) {
                    if stdout.write(chunk.to_vec()).await.is_err() {
                        // Attachment cancellation/exhaustion is arbitrated by
                        // the owner operation, not part of the remote result.
                        break;
                    }
                }
            }
            Ok(value)
        }
        Err(error) => Err(error),
    };
    if let Some(stdout) = invocation.stdout {
        if matches!(&result, Err(SerializableToolRpcError::Cancelled)) {
            // Replay must reconstruct the same terminal without relying on
            // the live cancellation observer winning the attachment race.
            let _ = stdout.writer.fail(ByteStreamFailure::Cancelled);
        } else {
            let _ = stdout.finish();
        }
    }
    Ok(result)
}

async fn tool_presence<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    activation: &McpImportActivation,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> anyhow::Result<Result<Option<bool>, SerializableToolRpcError>> {
    // A protocol rejection does not distinguish removal from invalid arguments.
    // Record the subsequent observation separately, after committing the call:
    // quota suspension or a crash repairs the read without resending tools/call
    // (unless an enclosing atomic region is rolled back).
    let mut call =
        DurableCallSession::<McpToolPresence, LeaveIncompleteOnDrop>::start_with_agent_authority(
            ctx,
            HostRequestMcpToolPresence {
                upstream_tool_name: activation.source.upstream_tool_name.clone(),
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;
    if !call.is_live() {
        match call.replay(ctx).await? {
            CallReplayOutcome::Replayed(response) => {
                return Ok(presence_observation(response.result));
            }
            CallReplayOutcome::Incomplete(live) => call = live,
        }
    }
    let auth = call.take_agent_auth_ctx();
    let mut source = activation.source.clone();
    source.upstream_tool_name.clear();
    let result = loop {
        let result = tokio::select! {
            biased;
            _ = async {
                match cancellation {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            } => {
                call.complete(ctx, HostResponseMcpToolPresence { result: Err(SerializableToolRpcError::Cancelled) }).await?;
                return Ok(Err(SerializableToolRpcError::Cancelled));
            }
            result = ctx.state.environment_state_service.resolve_mcp_import(&source, &auth, true) => result,
        };
        if matches!(&result, Err(RegistryServiceError::LimitExceeded(_))) {
            return Err(call.trap(GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted));
        }
        match call
            .try_trigger_retry_or_loop(ctx, &result, classify_registry_error)
            .await?
        {
            InternalRetryResult::RetryInternally => continue,
            InternalRetryResult::Persist => break result,
        }
    };
    let result = result
        .map(|observation| {
            let name = &activation.source.upstream_tool_name;
            observation
                .tools
                .iter()
                .any(|tool| &tool.upstream_name == name)
                || observation
                    .diagnostics
                    .iter()
                    .any(|diagnostic| &diagnostic.upstream_name == name)
        })
        .map_err(|error| registry_failure(&error));
    let response = call
        .complete(ctx, HostResponseMcpToolPresence { result })
        .await?;
    Ok(presence_observation(response.result))
}

fn presence_observation(
    result: Result<bool, SerializableToolRpcError>,
) -> Result<Option<bool>, SerializableToolRpcError> {
    match result {
        Err(SerializableToolRpcError::Cancelled) => Err(SerializableToolRpcError::Cancelled),
        result => Ok(result.ok()),
    }
}

async fn live_call<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    activation: &McpImportActivation,
    tool: &ProjectedTool,
    input: &TypedSchemaValue,
    auth: &AuthCtx,
    key: &str,
    unauthorized_generation: &mut Option<Option<uuid::Uuid>>,
) -> anyhow::Result<Value> {
    let service = ctx.state.environment_state_service.clone();
    let source = &activation.source;
    let deployment = service
        .get_tool_deployment_state_at_revision(source.environment_id, source.deployment_revision)
        .await?;
    let import = deployment
        .mcp_imports
        .get(source.import_index as usize)
        .ok_or_else(|| RegistryServiceError::NotFound("admitted MCP import is missing".into()))?;
    let credential = service.get_mcp_runtime_credential(source, auth).await?;
    let permit = ctx.public_state.worker().mcp_transport().acquire().await?;
    let client = Client::new(
        &import.url,
        Some(&activation.protocol_version),
        permit.limits()?,
    )?;
    let client = match credential.credential {
        Some(McpImportCredential::Bearer { token }) => client.with_bearer(&token)?,
        Some(McpImportCredential::Basic { user, password }) => {
            client.with_basic(&user, &password)?
        }
        None => client,
    };
    let mut sender = permit.sender(ctx);
    let result = client
        .call_tool(&mut sender, tool, input.value(), Some(key))
        .await;
    if matches!(&result, Err(error) if matches!(error.downcast_ref::<TransportError>(), Some(TransportError::AuthorizationRequired(401))))
    {
        *unauthorized_generation = Some(credential.oauth_grant_generation);
    }
    result
}

fn classify_error(error: &anyhow::Error) -> HostFailureKind {
    if let Some(error) = error.downcast_ref::<ToolDiscoveryError>() {
        return super::classify_tool_discovery_error(error);
    }
    match error.downcast_ref::<TransportError>() {
        Some(
            TransportError::Network
            | TransportError::Timeout
            | TransportError::HttpStatus(429 | 500..=599),
        ) => HostFailureKind::Transient,
        Some(_) => HostFailureKind::Permanent,
        None => error
            .downcast_ref::<RegistryServiceError>()
            .map_or(HostFailureKind::Permanent, classify_registry_error),
    }
}

fn classify_registry_error(error: &RegistryServiceError) -> HostFailureKind {
    match error {
        RegistryServiceError::InternalServerError(_)
        | RegistryServiceError::InternalClientError(_) => HostFailureKind::Transient,
        _ => HostFailureKind::Permanent,
    }
}

pub(super) fn registry_failure(error: &RegistryServiceError) -> SerializableToolRpcError {
    match error {
        RegistryServiceError::Unauthorized(_) | RegistryServiceError::CouldNotAuthenticate(_) => {
            SerializableToolRpcError::Denied(error.to_string())
        }
        _ => SerializableToolRpcError::RemoteInternalError(error.to_string()),
    }
}

async fn encode_response(response: Result<Value, TransportError>) -> anyhow::Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        stacker::maybe_grow(2 << 20, 64 << 20, || serde_json::to_vec(&response))
    })
    .await?
    .map_err(Into::into)
}

type ProjectedResponse =
    Result<(SerializableToolStructuredResult, Option<Vec<u8>>), SerializableToolRpcError>;

fn decode_response(bytes: &[u8]) -> anyhow::Result<Result<Value, TransportError>> {
    stacker::maybe_grow(2 << 20, 64 << 20, || {
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        decoder.disable_recursion_limit();
        let response = Result::<Value, TransportError>::deserialize(&mut decoder)?;
        decoder.end()?;
        Ok(response)
    })
}

fn project_response(
    tool: ProjectedTool,
    response: Result<Value, TransportError>,
    present: Option<bool>,
) -> anyhow::Result<ProjectedResponse> {
    stacker::maybe_grow(2 << 20, 64 << 20, || {
        let response = match response {
            Ok(response) => response,
            Err(TransportError::Remote { code: -32602, .. }) if present == Some(false) => {
                return Ok(Err(SerializableToolRpcError::RemoteToolError(Box::new(
                    SerializableToolError::InvalidToolName(tool.definition.name().unwrap().into()),
                ))));
            }
            Err(
                TransportError::InvalidInput(details)
                | TransportError::Remote {
                    code: -32602,
                    message: details,
                    ..
                },
            ) => return Ok(Err(tool_error(CallError::InvalidInput(details)))),
            Err(TransportError::Remote {
                code,
                message,
                data,
            }) => {
                let details = serde_json::json!({"code": code, "message": message, "data": data});
                return Ok(Err(tool_error(CallError::ToolError(details.to_string()))));
            }
            Err(TransportError::Denied) => {
                return Ok(Err(SerializableToolRpcError::Denied(
                    "MCP network access denied".into(),
                )));
            }
            Err(error) => {
                return Ok(Err(SerializableToolRpcError::RemoteInternalError(
                    error.to_string(),
                )));
            }
        };
        let projected = match tool.response(&response) {
            Ok(projected) => projected,
            Err(error) => return Ok(Err(tool_error(error))),
        };
        let root = tool.definition.commands.nodes[0]
            .body
            .as_ref()
            .and_then(|body| body.result.as_ref())
            .ok_or_else(|| anyhow::anyhow!("MCP projection has no result schema"))?
            .type_
            .clone();
        let graph = SchemaGraph {
            defs: reachable_defs(&tool.definition.schema, &root),
            root,
        };
        let value = TypedSchemaValue::new(graph, projected.value);
        Ok(Ok((
            SerializableToolStructuredResult {
                result: Some(
                    SerializableToolResultValue::from_typed(&value).map_err(anyhow::Error::msg)?,
                ),
            },
            projected.stdout,
        )))
    })
}

fn tool_error(error: CallError) -> SerializableToolRpcError {
    SerializableToolRpcError::RemoteToolError(Box::new(match error {
        CallError::InvalidInput(details) => SerializableToolError::InvalidInput(details),
        CallError::InvalidResult(details) => SerializableToolError::InvalidResult(details),
        CallError::ToolError(details) => {
            SerializableToolError::CustomError(Box::new(SerializableCustomToolError {
                name: "mcp-tool-error".to_string(),
                payload: TypedSchemaValue::new(
                    SchemaGraph::anonymous(SchemaType::string()),
                    SchemaValue::String(details),
                ),
            }))
        }
    }))
}

#[cfg(test)]
mod tests;
