#[cfg(test)]
test_r::enable!();

mod model;

use futures_concurrency::prelude::*;
use golem_rust::agentic::get_agent_id;
use golem_rust::schema::{FromSchema, IntoSchema};
use golem_rust::tool::{
    InputStream, InvocationResult, OutputStream, Principal, RawCustomToolError, Tool,
    ToolInvokeError, UnderlyingTool,
};
use golem_rust::{TypedSchemaValue, universal_tool_middleware};
use model::{
    AuditRecord, OutcomeSummary, OwnerSummary, PrincipalSummary, RECORD_VERSION, StreamSummary,
    StreamTerminal, summarize_value,
};
use serde::Serialize;
use std::time::Duration;

#[derive(IntoSchema, FromSchema)]
struct AuditParameters {
    label: String,
    sink_url: String,
}

#[universal_tool_middleware(
    name = "audit",
    version = "0.1.0",
    parameters = AuditParameters
)]
async fn audit(
    parameters: AuditParameters,
    tool_name: String,
    _tool_metadata: Tool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    principal: Principal,
    underlying: UnderlyingTool,
) -> Result<InvocationResult, ToolInvokeError<RawCustomToolError>> {
    let policy_invocation_key = golem_rust::generate_idempotency_key().to_string();
    let input_summary = summarize_value(input.value());
    let owner = owner_summary();
    let principal = principal_summary(principal);
    let (result, stdout, stderr) = invoke_and_summarize(
        underlying,
        command_path.clone(),
        input,
        stdin,
        stdout,
        stderr,
    )
    .await;
    let outcome = outcome_summary(&result);
    let record = AuditRecord {
        version: RECORD_VERSION,
        policy_invocation_key: policy_invocation_key.clone(),
        occurrence_label: parameters.label,
        tool_name,
        command_path,
        owner,
        principal,
        input: input_summary,
        outcome,
        stdout,
        stderr,
    };
    commit_record(&parameters.sink_url, &policy_invocation_key, &record).await?;
    result.map(|result| InvocationResult {
        result,
        stdout: None,
        stderr: None,
    })
}

async fn invoke_and_summarize(
    underlying: UnderlyingTool,
    command_path: Vec<String>,
    input: TypedSchemaValue,
    stdin: Option<InputStream>,
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
) -> (
    Result<Option<TypedSchemaValue>, ToolInvokeError<RawCustomToolError>>,
    StreamSummary,
    StreamSummary,
) {
    let mut invocation = match underlying.start(command_path, input, stdin).await {
        Ok(invocation) => invocation,
        Err(error) => return (Err(error), StreamSummary::absent(), StreamSummary::absent()),
    };
    let stdout = forward_and_summarize("stdout", invocation.stdout.take(), stdout);
    let stderr = forward_and_summarize("stderr", invocation.stderr.take(), stderr);
    let (result, stdout, stderr) = (invocation.get(), stdout, stderr).join().await;
    let (stdout_summary, stdout_result) = stdout;
    let (stderr_summary, stderr_result) = stderr;
    let result = match result {
        Err(error) => Err(error),
        Ok(value) => match stdout_result.and(stderr_result) {
            Ok(()) => Ok(value),
            Err(error) => Err(error),
        },
    };
    (result, stdout_summary, stderr_summary)
}

async fn forward_and_summarize(
    channel: &'static str,
    mut source: Option<InputStream>,
    mut destination: Option<OutputStream>,
) -> (
    StreamSummary,
    Result<(), ToolInvokeError<RawCustomToolError>>,
) {
    let Some(mut source) = source.take() else {
        return match destination {
            None => (StreamSummary::absent(), Ok(())),
            Some(destination) => {
                let mut summary = StreamSummary::declared();
                let result = classify_write(channel, destination.finish().await, &mut summary);
                (summary, result)
            }
        };
    };
    let mut summary = StreamSummary::declared();
    while let Some(item) = source.next().await {
        match item {
            Ok(bytes) => {
                summary.chunks += 1;
                summary.bytes += bytes.len() as u64;
                if let Some(writer) = destination.as_mut() {
                    let result = writer.write(bytes).await;
                    if let Err(error) = classify_write(channel, result, &mut summary) {
                        return (summary, Err(error));
                    }
                    if summary.terminal == StreamTerminal::ConsumerCancelled {
                        return (summary, Ok(()));
                    }
                }
            }
            Err(reason) => {
                summary.terminal = match &reason {
                    golem_rust::golem_agentic::golem::tool::streams::ByteStreamFailure::Cancelled => StreamTerminal::Cancelled,
                    golem_rust::golem_agentic::golem::tool::streams::ByteStreamFailure::Abandoned => StreamTerminal::Abandoned,
                    golem_rust::golem_agentic::golem::tool::streams::ByteStreamFailure::ResourceExhausted => {
                        StreamTerminal::ResourceExhausted
                    }
                    golem_rust::golem_agentic::golem::tool::streams::ByteStreamFailure::Failed(_) => StreamTerminal::Failed,
                };
                return match destination {
                    None => (summary, Ok(())),
                    Some(destination) => {
                        let result =
                            classify_write(channel, destination.fail(reason).await, &mut summary);
                        (summary, result)
                    }
                };
            }
        }
    }
    match destination {
        None => (summary, Ok(())),
        Some(destination) => {
            let result = classify_write(channel, destination.finish().await, &mut summary);
            (summary, result)
        }
    }
}

fn classify_write(
    channel: &str,
    result: Result<(), golem_rust::golem_agentic::golem::tool::streams::StreamWriteError>,
    summary: &mut StreamSummary,
) -> Result<(), ToolInvokeError<RawCustomToolError>> {
    use golem_rust::golem_agentic::golem::tool::streams::{ByteStreamCloseCause, StreamWriteError};
    match result {
        Ok(()) => Ok(()),
        Err(StreamWriteError::Closed(ByteStreamCloseCause::ConsumerCancelled)) => {
            summary.terminal = StreamTerminal::ConsumerCancelled;
            Ok(())
        }
        Err(error) => {
            summary.terminal = StreamTerminal::ForwardingFailed;
            Err(ToolInvokeError::InvalidResult(format!(
                "failed to forward underlying {channel}: {error:?}"
            )))
        }
    }
}

fn outcome_summary(
    result: &Result<Option<TypedSchemaValue>, ToolInvokeError<RawCustomToolError>>,
) -> OutcomeSummary {
    match result {
        Ok(result) => OutcomeSummary::Success {
            result: result.as_ref().map(|value| summarize_value(value.value())),
        },
        Err(error) => {
            let (error_kind, custom_name) = match error {
                ToolInvokeError::InvalidToolName(_) => ("invalid-tool-name", None),
                ToolInvokeError::InvalidCommandPath(_) => ("invalid-command-path", None),
                ToolInvokeError::InvalidInput(_) => ("invalid-input", None),
                ToolInvokeError::ConstraintViolation(_) => ("constraint-violation", None),
                ToolInvokeError::InvalidResult(_) => ("invalid-result", None),
                ToolInvokeError::Tool(error) | ToolInvokeError::UnknownCustomError(error) => {
                    ("tool", Some(error.name.clone()))
                }
                ToolInvokeError::ProtocolError(_) => ("protocol-error", None),
                ToolInvokeError::Denied(_) => ("denied", None),
                ToolInvokeError::InternalError(_) => ("internal-error", None),
                ToolInvokeError::Cancelled => ("cancelled", None),
                ToolInvokeError::ResourceExhausted(_) => ("resource-exhausted", None),
            };
            OutcomeSummary::Error {
                error_kind: error_kind.to_string(),
                custom_name,
            }
        }
    }
}

fn owner_summary() -> OwnerSummary {
    let owner = get_agent_id();
    OwnerSummary {
        component_id: owner.component_id.uuid.to_string(),
        agent_id: owner.agent_id,
    }
}

fn principal_summary(principal: Principal) -> PrincipalSummary {
    match principal {
        Principal::Anonymous => PrincipalSummary::Anonymous,
        Principal::Oidc(oidc) => PrincipalSummary::Oidc {
            issuer: oidc.issuer,
            subject: oidc.sub,
        },
        Principal::Agent(agent) => PrincipalSummary::Agent {
            component_id: golem_rust::Uuid::from_u64_pair(
                agent.agent_id.component_id.uuid.high_bits,
                agent.agent_id.component_id.uuid.low_bits,
            )
            .to_string(),
            agent_id: agent.agent_id.agent_id,
        },
        Principal::GolemUser(user) => PrincipalSummary::GolemUser {
            account_id: golem_rust::Uuid::from_u64_pair(
                user.account_id.uuid.high_bits,
                user.account_id.uuid.low_bits,
            )
            .to_string(),
        },
    }
}

async fn commit_record(
    sink_url: &str,
    key: &str,
    record: &impl Serialize,
) -> Result<(), ToolInvokeError<RawCustomToolError>> {
    let body = serde_json::to_vec(record)
        .map_err(|error| ToolInvokeError::InternalError(format!("encode audit record: {error}")))?;
    let response = wasi_fetch::Client::new()
        .post(sink_url)
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .timeout(Duration::from_secs(10))
        .between_bytes_timeout(Duration::from_secs(10))
        .body(body)
        .send()
        .await
        .map_err(|error| {
            ToolInvokeError::InternalError(format!("audit sink request failed: {error}"))
        })?;
    let status = response.status().as_u16();
    let _ = response.into_body().bytes().await;
    if !(200..300).contains(&status) {
        return Err(ToolInvokeError::InternalError(format!(
            "audit sink rejected record with HTTP status {status}"
        )));
    }
    Ok(())
}
