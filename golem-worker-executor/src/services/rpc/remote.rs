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

use super::*;

impl RemoteInvocationRpc {
    pub fn new(worker_proxy: Arc<dyn WorkerProxy>, shard_service: Arc<dyn ShardService>) -> Self {
        Self {
            worker_proxy,
            _shard_service: shard_service,
        }
    }
}

/// Rpc implementation simply calling the public Golem Worker API for invocation
#[async_trait]
impl Rpc for RemoteInvocationRpc {
    async fn create_demand(
        &self,
        owned_agent_id: &OwnedAgentId,
        method_name: &str,
        _self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> Result<Box<dyn RpcDemand>, RpcError> {
        debug!("Ensuring remote target worker exists");

        let principal = caller_agent_principal(self_agent_id);

        let fingerprint = self
            .worker_proxy
            .prepare(
                owned_agent_id,
                method_name,
                self_agent_id,
                HashMap::from_iter(self_env.to_vec()),
                self_stack,
                config,
                principal,
                auth_ctx,
                idempotency_key,
                freshness_disposition,
            )
            .await?;

        Ok(Box::new(LoggingDemand::new(
            owned_agent_id.agent_id(),
            fingerprint,
        )))
    }

    async fn invoke_and_await(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        method_name: String,
        method_parameters: SchemaValue,
        _self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        scope_card: Option<ScopeCard>,
    ) -> Result<SchemaValue, RpcError> {
        let principal = caller_agent_principal(self_agent_id);

        let output = self
            .worker_proxy
            .invoke_agent(
                &owned_agent_id.agent_id(),
                method_name,
                method_parameters,
                AgentInvocationMode::Await,
                None,
                idempotency_key,
                freshness_disposition,
                self_agent_id.clone(),
                HashMap::from_iter(self_env.to_vec()),
                self_stack,
                config,
                principal,
                owned_agent_id.environment_id,
                auth_ctx,
                scope_card,
            )
            .await?;

        match output.result {
            golem_common::model::AgentInvocationResult::AgentMethod { output } => Ok(output),
            _ => Err(RpcError::RemoteInternalError {
                details:
                    "Expected a result from agent invoke_and_await but got a non-method result"
                        .to_string(),
            }),
        }
    }

    async fn invoke_and_await_streaming(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: IdempotencyKey,
        method_name: String,
        method_parameters: ProtoSchemaValue,
        input_mappings: Vec<DurableStreamMapping>,
        expected_callee_fingerprint: AgentFingerprint,
        attempt_id: uuid::Uuid,
        origin_invocation: StreamInvocationIdentity,
        accepted_inputs: tokio::sync::oneshot::Sender<InvocationAccepted>,
        _self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
        scope_card: Option<ScopeCard>,
    ) -> Result<DurableRpcInvocationResult, RpcError> {
        let start = InvocationRequest {
            request: Some(invocation_request::Request::Start(InvocationStart {
                agent_id: Some(owned_agent_id.agent_id().into()),
                method_name: Some(method_name),
                input: Some(method_parameters),
                idempotency_key: Some(idempotency_key.into()),
                context: Some(golem_api_grpc::proto::golem::worker::InvocationContext {
                    parent: Some(self_agent_id.clone().into()),
                    env: HashMap::from_iter(self_env.to_vec()),
                    tracing: Some(self_stack.into()),
                }),
                auth_ctx: Some(auth_ctx.clone().into()),
                principal: Some(caller_agent_principal(self_agent_id).into()),
                environment_id: Some(owned_agent_id.environment_id.into()),
                config: config.into_iter().map(Into::into).collect(),
                component_owner_account_id: None,
                mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                schedule_at: None,
                freshness_disposition:
                    golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                        as i32,
                attempt_id: Some(attempt_id.into()),
                expected_callee_fingerprint: Some(expected_callee_fingerprint.0.into()),
                durable_input_mappings: input_mappings,
                origin_invocation: Some(origin_invocation),
                scope_card: scope_card
                    .as_ref()
                    .map(golem_api_grpc::proto::golem::worker::EncodedScopeCard::try_from)
                    .transpose()
                    .map_err(|details| RpcError::ProtocolError { details })?,
                external_tool: None,
            })),
        };
        let mut accepted_inputs = Some(accepted_inputs);
        let mut retry_delay = std::time::Duration::from_millis(25);
        let mut request = start;
        let mut attachment_state_retries = 0;
        loop {
            let state = Arc::new(tokio::sync::Mutex::new(InvocationSessionState::default()));
            state
                .lock()
                .await
                .validate_trusted_request(&request)
                .map_err(|details| RpcError::ProtocolError { details })?;
            let (requests, receiver) = mpsc::channel(2);
            if requests.send(request.clone()).await.is_err() {
                tracing::warn!(
                    "retrying durable RPC attachment request after the local request stream closed"
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(1));
                continue;
            }
            let mut inbound = match self
                .worker_proxy
                .invoke_agent_session(Box::pin(ReceiverStream::new(receiver)))
                .await
            {
                Ok(inbound) => inbound,
                Err(error) => {
                    tracing::warn!(%error, "retrying durable RPC attachment request after transport establishment failed");
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(1));
                    continue;
                }
            };
            let _requests = requests;
            let mut retry_reason = None;

            while let Some(response) = inbound.next().await {
                let response = match response {
                    Ok(response) => response,
                    Err(error) => {
                        retry_reason = Some(error.to_string());
                        break;
                    }
                };
                state
                    .lock()
                    .await
                    .validate_response(&response)
                    .map_err(|details| RpcError::ProtocolError { details })?;
                match response.response {
                    Some(invocation_response::Response::Accepted(accepted)) => {
                        attachment_state_retries = 0;
                        if let Some(sender) = accepted_inputs.take() {
                            let _ = sender.send(accepted.clone());
                        }
                        if accepted.attachment_id.is_some() {
                            // Keep this exact attempt on ambiguous loss; only a new acceptance
                            // supplies the epoch for the next resume attempt.
                            request = InvocationRequest {
                                request: Some(invocation_request::Request::ResumeAttach(
                                    ResumeAttach {
                                        idempotency_key: accepted.idempotency_key.clone(),
                                        agent_id: accepted.agent_id.clone(),
                                        environment_id: accepted.environment_id,
                                        attachment_id: accepted.attachment_id,
                                        expected_callee_fingerprint: accepted.callee_fingerprint,
                                        expected_epoch: accepted.epoch,
                                        attempt_id: Some(uuid::Uuid::new_v4().into()),
                                        operation: ResumeOperation::Resume as i32,
                                        cursors: Vec::new(),
                                        auth_ctx: Some(auth_ctx.clone().into()),
                                        principal: Some(
                                            caller_agent_principal(self_agent_id).into(),
                                        ),
                                    },
                                )),
                            };
                        }
                    }
                    Some(invocation_response::Response::Rejected(rejected)) => {
                        confirm_terminal_response_is_last(&mut inbound, &state).await?;
                        if let Some(invocation_request::Request::ResumeAttach(resume)) =
                            &mut request.request
                            && attachment_state_retries < 2
                            && InvocationRejectionReason::try_from(rejected.reason)
                                == Ok(InvocationRejectionReason::InvalidAttachmentState)
                        {
                            // At one epoch an attached transport can detach once; reattachment
                            // advances the epoch and is rejected separately as stale.
                            resume.operation = if resume.operation == ResumeOperation::Resume as i32
                            {
                                ResumeOperation::Takeover as i32
                            } else {
                                ResumeOperation::Resume as i32
                            };
                            resume.attempt_id = Some(uuid::Uuid::new_v4().into());
                            attachment_state_retries += 1;
                            retry_reason = Some("attachment state changed".to_string());
                            break;
                        }
                        return Err(rpc_error_from_rejection(rejected));
                    }
                    Some(invocation_response::Response::Result(invocation_result)) => {
                        let value = match invocation_result.result {
                            Some(invocation_session_result::Result::MethodResult(output)) => output,
                            Some(invocation_session_result::Result::NoResult(_)) | None => {
                                return Err(RpcError::ProtocolError {
                                    details:
                                        "durable streaming invocation returned no method result"
                                            .to_string(),
                                });
                            }
                            Some(invocation_session_result::Result::ToolResult(_)) => {
                                return Err(RpcError::ProtocolError {
                                    details: "agent RPC returned an external-tool result"
                                        .to_string(),
                                });
                            }
                        };
                        let result = DurableRpcInvocationResult {
                            value,
                            output_mappings: invocation_result.new_stream_mappings,
                        };
                        let drain_state = state.clone();
                        wasmtime_wasi::runtime::spawn(async move {
                            while let Some(response) = inbound.next().await {
                                match response {
                                    Ok(response) => {
                                        if let Err(details) =
                                            drain_state.lock().await.validate_response(&response)
                                        {
                                            tracing::warn!(%details, "invalid response while draining durable RPC completion");
                                            break;
                                        }
                                        if matches!(
                                            response.response,
                                            Some(invocation_response::Response::Finished(_))
                                                | Some(invocation_response::Response::Rejected(_))
                                        ) {
                                            break;
                                        }
                                    }
                                    Err(error) => {
                                        tracing::warn!(%error, "durable RPC completion drain failed");
                                        break;
                                    }
                                }
                            }
                        });
                        return Ok(result);
                    }
                    Some(invocation_response::Response::Finished(finished)) => {
                        confirm_terminal_response_is_last(&mut inbound, &state).await?;
                        return Err(match finished.outcome {
                            Some(invocation_session_completion::Outcome::Success(_)) => {
                                RpcError::ProtocolError {
                                    details: "durable invocation completed without a result"
                                        .to_string(),
                                }
                            }
                            _ => rpc_error_from_invocation_finished(finished),
                        });
                    }
                    Some(invocation_response::Response::AttachmentRevoked(_)) => {
                        retry_reason = Some("attachment revoked".to_string());
                        break;
                    }
                    Some(invocation_response::Response::OutputItem(_))
                    | Some(invocation_response::Response::OutputEnd(_))
                    | Some(invocation_response::Response::OutputError(_))
                    | Some(invocation_response::Response::InputAck(_))
                    | Some(invocation_response::Response::StreamCancel(_)) => {}
                    None => unreachable!("response state validation rejects empty frames"),
                }
            }
            tracing::warn!(
                reason = retry_reason
                    .as_deref()
                    .unwrap_or("durable invocation response ended before publishing a result"),
                "retrying durable RPC attachment after ambiguous response loss"
            );
            tokio::time::sleep(retry_delay).await;
            retry_delay = (retry_delay * 2).min(std::time::Duration::from_secs(1));
        }
    }

    async fn control_durable_stream_attachment(
        &self,
        request: StreamAttachmentControlRequest,
        auth_ctx: &AuthCtx,
    ) -> Result<bool, DurableStreamRemoteError<RpcError>> {
        self.worker_proxy
            .control_durable_stream_attachment(request, auth_ctx)
            .await
            .map_err(|error| error.map_other(RpcError::from))
    }

    async fn read_durable_stream_segment(
        &self,
        request: DurableStreamReadRequest,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<u8>, DurableStreamRemoteError<RpcError>> {
        self.worker_proxy
            .read_durable_stream_segment(request, auth_ctx)
            .await
            .map_err(|error| error.map_other(Into::into))
    }

    async fn invoke(
        &self,
        owned_agent_id: &OwnedAgentId,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        method_name: String,
        method_parameters: SchemaValue,
        _self_created_by: AccountId,
        self_agent_id: &AgentId,
        self_env: &[(String, String)],
        self_stack: InvocationContextStack,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), RpcError> {
        let principal = caller_agent_principal(self_agent_id);

        self.worker_proxy
            .invoke_agent(
                &owned_agent_id.agent_id(),
                method_name,
                method_parameters,
                AgentInvocationMode::Schedule,
                None,
                idempotency_key,
                freshness_disposition,
                self_agent_id.clone(),
                HashMap::from_iter(self_env.to_vec()),
                self_stack,
                config,
                principal,
                owned_agent_id.environment_id,
                auth_ctx,
                None,
            )
            .await?;

        Ok(())
    }
}

async fn confirm_terminal_response_is_last(
    inbound: &mut InvocationResponseStream,
    state: &Arc<tokio::sync::Mutex<InvocationSessionState>>,
) -> Result<(), RpcError> {
    match inbound.next().await {
        None => Ok(()),
        Some(Err(error)) => Err(error.into()),
        Some(Ok(response)) => {
            let details = state.lock().await.validate_response(&response).unwrap_err();
            Err(RpcError::ProtocolError { details })
        }
    }
}

fn rpc_error_from_rejection(rejected: InvocationRejected) -> RpcError {
    match InvocationRejectionReason::try_from(rejected.reason)
        .unwrap_or(InvocationRejectionReason::Internal)
    {
        InvocationRejectionReason::Unauthorized => RpcError::Denied {
            details: rejected.error,
        },
        InvocationRejectionReason::NotFound => RpcError::NotFound {
            details: rejected.error,
        },
        InvocationRejectionReason::Internal => RpcError::RemoteInternalError {
            details: rejected.error,
        },
        _ => RpcError::ProtocolError {
            details: rejected.error,
        },
    }
}

fn rpc_error_from_invocation_finished(
    finished: golem_api_grpc::proto::golem::worker::InvocationSessionCompletion,
) -> RpcError {
    match finished.outcome {
        Some(invocation_session_completion::Outcome::Failure(failure)) => {
            rpc_error_from_failure(failure)
        }
        Some(invocation_session_completion::Outcome::Success(_)) => RpcError::ProtocolError {
            details: "invocation completed successfully before publishing a result".to_string(),
        },
        None => RpcError::ProtocolError {
            details: "invocation completion has no outcome".to_string(),
        },
    }
}
