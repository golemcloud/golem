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

use super::WorkerResult;
use super::{
    AllExecutors, CallWorkerExecutorError, HasWorkerExecutorClients, RandomExecutor,
    ResponseMapResult, RoutingLogic, WorkerServiceError, WorkerStream,
};
use crate::service::auth::AuthServiceError;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::TryStreamExt;
use futures::{Stream, StreamExt};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::worker::invocation_request;
use golem_api_grpc::proto::golem::worker::invocation_response;
use golem_api_grpc::proto::golem::worker::invocation_session_completion;
use golem_api_grpc::proto::golem::worker::invocation_session_result;
use golem_api_grpc::proto::golem::worker::{
    InvocationContext, InvocationFailure, InvocationFailureKind, InvocationRejected,
    InvocationRejectionReason, InvocationRequest, InvocationResponse, InvocationSessionResult,
    InvocationStart, LogEvent,
};
use golem_api_grpc::proto::golem::workerexecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    ActivatePluginRequest, CancelInvocationRequest, CompletePromiseRequest, ConnectWorkerRequest,
    CreateWorkerRequest, DeactivatePluginRequest, ForkWorkerRequest, InterruptWorkerRequest,
    ProcessOplogEntriesRequest, ResumeWorkerRequest, RevertWorkerRequest, SearchOplogResponse,
    UpdateWorkerRequest,
};
use golem_common::model::RetryConfig;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::card::StoredCard;
use golem_common::model::component::{
    CanonicalFilePath, ComponentId, ComponentRevision, PluginPriority,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogCursor, PublicOplogEntry};
use golem_common::model::oplog::{OplogIndex, PublicOplogEntryWithIndex};
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::worker::AgentUpdateMode;
use golem_common::model::worker::{AgentMetadataDto, RevertWorkerTarget};
use golem_common::model::{
    AgentFilter, AgentFingerprint, AgentId, AgentStatus, FilterComparator, IdempotencyKey,
    PromiseId, ScanCursor,
};
use golem_common::model::{AgentInvocationOutput, AgentInvocationResult, InvocationStatus};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::grpc::client::MultiTargetGrpcClient;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::{ComponentFileSystemNode, GetOplogResponse};
use golem_service_base::service::routing_table::{HasRoutingTableService, RoutingTableService};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;
use tonic::{Code, Status};
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

fn freshness_disposition_for_dispatch(
    requested: InvocationFreshnessDisposition,
    first_dispatch: &AtomicBool,
) -> InvocationFreshnessDisposition {
    if requested == InvocationFreshnessDisposition::KnownFresh
        && first_dispatch.swap(false, Ordering::AcqRel)
    {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    }
}

pub type InvocationRequestStream = Pin<Box<dyn Stream<Item = InvocationRequest> + Send + 'static>>;
pub type InvocationResponseStream =
    Pin<Box<dyn Stream<Item = Result<InvocationResponse, Status>> + Send + 'static>>;
type InvocationSessionCall<'a> = Pin<
    Box<
        dyn Future<Output = Result<tonic::Response<tonic::Streaming<InvocationResponse>>, Status>>
            + Send
            + 'a,
    >,
>;

fn invoke_agent_session_once<'a>(
    client: &'a mut WorkerExecutorClient<OtelGrpcService<Channel>>,
    request: Option<InvocationRequestStream>,
) -> InvocationSessionCall<'a> {
    match request {
        Some(request) => Box::pin(client.invoke_agent_session(request)),
        None => Box::pin(std::future::ready(Err(Status::aborted(
            "invocation session request was already consumed",
        )))),
    }
}

#[derive(Debug)]
enum OneShotInvocationSessionResult {
    Success(AgentInvocationOutput),
    Rejected(InvocationRejected),
    Failure(InvocationFailure),
    ProtocolFailure(String),
}

fn protocol_failure(details: impl Into<String>) -> OneShotInvocationSessionResult {
    OneShotInvocationSessionResult::ProtocolFailure(details.into())
}

fn decode_invocation_rejection(rejected: InvocationRejected) -> WorkerServiceError {
    match InvocationRejectionReason::try_from(rejected.reason) {
        Ok(InvocationRejectionReason::NotFound) => rejected
            .agent_id
            .map(TryInto::try_into)
            .transpose()
            .map_err(WorkerServiceError::TypeChecker)
            .and_then(|agent_id| {
                agent_id
                    .map(WorkerServiceError::AgentNotFound)
                    .ok_or_else(|| WorkerServiceError::Internal(rejected.error.clone()))
            })
            .unwrap_or_else(|error| error),
        Ok(InvocationRejectionReason::Unauthorized) => {
            WorkerServiceError::AuthError(AuthServiceError::CouldNotAuthenticate)
        }
        Ok(InvocationRejectionReason::Internal) => WorkerServiceError::Internal(rejected.error),
        _ => WorkerServiceError::TypeChecker(rejected.error),
    }
}

fn decode_invocation_failure(failure: InvocationFailure) -> WorkerExecutorError {
    if failure.kind == InvocationFailureKind::Protocol as i32 {
        WorkerExecutorError::invalid_request(failure.message)
    } else if let Some(worker_error) = failure.worker_error {
        worker_error
            .try_into()
            .unwrap_or_else(|error| WorkerExecutorError::Unknown {
                details: format!("failed to decode worker execution error: {error}"),
            })
    } else {
        WorkerExecutorError::Unknown {
            details: failure.message,
        }
    }
}

fn decode_invocation_result(
    wire: InvocationSessionResult,
) -> Result<AgentInvocationOutput, String> {
    let result = match wire.result.ok_or("invocation result has no payload")? {
        invocation_session_result::Result::MethodResult(value) => {
            AgentInvocationResult::AgentMethod {
                output: value.try_into()?,
            }
        }
        invocation_session_result::Result::NoResult(_) => {
            AgentInvocationResult::AgentInitialization
        }
    };
    let invocation_status = wire.status.and_then(|status| {
        golem_api_grpc::proto::golem::worker::InvocationStatus::try_from(status)
            .ok()
            .map(InvocationStatus::from)
    });
    Ok(AgentInvocationOutput {
        result,
        consumed_fuel: wire.fuel_consumed,
        invocation_status,
        component_revision: wire
            .component_revision
            .map(ComponentRevision::new)
            .transpose()
            .map_err(|error| error.to_string())?,
        agent_id: wire.agent_id.map(TryInto::try_into).transpose()?,
        idempotency_key: wire.idempotency_key.map(Into::into),
        oplog_index: wire.oplog_index.map(OplogIndex::from_u64),
        agent_fingerprint: wire
            .agent_fingerprint
            .map(|uuid| AgentFingerprint(uuid.into())),
    })
}

async fn run_one_shot_invocation_session(
    client: &mut WorkerExecutorClient<OtelGrpcService<Channel>>,
    start: InvocationStart,
) -> Result<OneShotInvocationSessionResult, Status> {
    let (requests, receiver) = mpsc::channel(1);
    let request = InvocationRequest {
        request: Some(invocation_request::Request::Start(start)),
    };
    let mut state = InvocationSessionState::default();
    state
        .validate_trusted_request(&request)
        .map_err(Status::invalid_argument)?;
    requests
        .send(request)
        .await
        .map_err(|_| Status::unavailable("invocation session request ended before start"))?;
    let responses = client
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    collect_one_shot_invocation_session(responses, state).await
}

async fn collect_one_shot_invocation_session<S>(
    mut responses: S,
    mut state: InvocationSessionState,
) -> Result<OneShotInvocationSessionResult, Status>
where
    S: Stream<Item = Result<InvocationResponse, Status>> + Unpin,
{
    let mut result = None;
    let mut terminal_outcome = None;

    while let Some(response) = responses.next().await.transpose()? {
        if let Err(details) = state.validate_response(&response) {
            return Ok(protocol_failure(details));
        }
        match response.response {
            Some(invocation_response::Response::Accepted(_)) => {}
            Some(invocation_response::Response::Rejected(rejected)) => {
                terminal_outcome = Some(Ok(OneShotInvocationSessionResult::Rejected(rejected)));
            }
            Some(invocation_response::Response::Result(invocation_result)) => {
                result = match decode_invocation_result(invocation_result) {
                    Ok(result) => Some(result),
                    Err(details) => return Ok(protocol_failure(details)),
                };
            }
            Some(invocation_response::Response::Finished(finished)) => {
                terminal_outcome = Some(match finished.outcome {
                    Some(invocation_session_completion::Outcome::Success(_)) => result
                        .take()
                        .map(OneShotInvocationSessionResult::Success)
                        .ok_or_else(|| Status::internal("invocation completed without a result")),
                    Some(invocation_session_completion::Outcome::Failure(failure)) => {
                        if failure.kind == InvocationFailureKind::Transport as i32 {
                            Err(Status::unavailable(failure.message))
                        } else {
                            Ok(OneShotInvocationSessionResult::Failure(failure))
                        }
                    }
                    None => Ok(protocol_failure("invocation completion has no outcome")),
                });
            }
            Some(
                invocation_response::Response::OutputItem(_)
                | invocation_response::Response::OutputEnd(_)
                | invocation_response::Response::OutputError(_)
                | invocation_response::Response::InputAck(_)
                | invocation_response::Response::StreamCancel(_),
            ) => {
                return Ok(protocol_failure(
                    "a non-streaming invocation received a stream frame",
                ));
            }
            Some(invocation_response::Response::AttachmentRevoked(_)) => {
                unreachable!("response validation rejects attachment revocation")
            }
            None => unreachable!("response validation rejects empty frames"),
        }
    }

    terminal_outcome.unwrap_or_else(|| {
        Err(Status::unavailable(
            "invocation session response ended before completion",
        ))
    })
}

#[async_trait]
pub trait WorkerClient: Send + Sync {
    async fn create(
        &self,
        agent_id: &AgentId,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        account_id: AccountId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
        invocation_context: Option<InvocationContext>,
        principal: Option<golem_api_grpc::proto::golem::component::Principal>,
    ) -> WorkerResult<(AgentId, AgentFingerprint)>;

    async fn connect(
        &self,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<WorkerStream<LogEvent>>;

    async fn delete(
        &self,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn complete_promise(
        &self,
        agent_id: &AgentId,
        oplog_id: u64,
        data: Vec<u8>,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool>;

    async fn interrupt(
        &self,
        agent_id: &AgentId,
        recover_immediately: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn get_metadata(
        &self,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<AgentMetadataDto>;

    async fn find_metadata(
        &self,
        component_id: ComponentId,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<AgentMetadataDto>)>;

    async fn resume(
        &self,
        agent_id: &AgentId,
        force: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn update(
        &self,
        agent_id: &AgentId,
        update_mode: AgentUpdateMode,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn get_oplog(
        &self,
        agent_id: &AgentId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError>;

    async fn search_oplog(
        &self,
        agent_id: &AgentId,
        cursor: Option<OplogCursor>,
        count: u64,
        query: String,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError>;

    async fn get_file_system_node(
        &self,
        agent_id: &AgentId,
        path: CanonicalFilePath,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<ComponentFileSystemNode>>;

    async fn get_agent_wallet(
        &self,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<StoredCard>>;

    async fn get_file_contents(
        &self,
        agent_id: &AgentId,
        path: CanonicalFilePath,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>>;

    async fn activate_plugin(
        &self,
        agent_id: &AgentId,
        plugin_priority: PluginPriority,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn deactivate_plugin(
        &self,
        agent_id: &AgentId,
        plugin_priority: PluginPriority,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn fork_worker(
        &self,
        source_agent_id: &AgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        environment_id: EnvironmentId,
        account_id: AccountId,
        account_email: AccountEmail,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn revert_worker(
        &self,
        agent_id: &AgentId,
        target: RevertWorkerTarget,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;

    async fn cancel_invocation(
        &self,
        agent_id: &AgentId,
        idempotency_key: &IdempotencyKey,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool>;

    async fn invoke_agent(
        &self,
        agent_id: &AgentId,
        method_name: Option<String>,
        method_parameters: Option<golem_api_grpc::proto::golem::schema::SchemaValue>,
        mode: i32,
        schedule_at: Option<::prost_types::Timestamp>,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: Option<InvocationContext>,
        freshness_disposition: InvocationFreshnessDisposition,
        config: Vec<AgentConfigEntryDto>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
        principal: golem_api_grpc::proto::golem::component::Principal,
    ) -> WorkerResult<AgentInvocationOutput>;

    async fn invoke_agent_session(
        &self,
        _agent_id: &AgentId,
        _request: InvocationRequestStream,
    ) -> WorkerResult<InvocationResponseStream> {
        Err(WorkerServiceError::Internal(
            "invocation sessions are not supported by this worker client".to_string(),
        ))
    }

    async fn process_oplog_entries(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        component_revision: ComponentRevision,
        idempotency_key: IdempotencyKey,
        account_id: AccountId,
        config: std::collections::HashMap<String, String>,
        metadata: golem_api_grpc::proto::golem::worker::AgentMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()>;
}

#[derive(Clone)]
pub struct WorkerExecutorWorkerClient {
    worker_executor_clients: MultiTargetGrpcClient<WorkerExecutorClient<OtelGrpcService<Channel>>>,
    // NOTE: unlike other retries, reaching max_attempts for the worker executor
    //       (with retryable errors) does not end the retry loop,
    //       rather it emits a warn log and resets the retry state.
    worker_executor_retries: RetryConfig,
    routing_table_service: Arc<RoutingTableService>,
}

impl WorkerExecutorWorkerClient {
    pub fn new(
        worker_executor_clients: MultiTargetGrpcClient<
            WorkerExecutorClient<OtelGrpcService<Channel>>,
        >,
        worker_executor_retries: RetryConfig,
        routing_table_service: Arc<RoutingTableService>,
    ) -> Self {
        Self {
            worker_executor_clients,
            worker_executor_retries,
            routing_table_service,
        }
    }

    async fn find_running_metadata_internal(
        &self,
        component_id: ComponentId,
        filter: Option<AgentFilter>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<AgentMetadataDto>> {
        let result = self.call_worker_executor(
            AllExecutors,
            "get_running_workers_metadata",
            move |worker_executor_client| {
                let component_id: golem_api_grpc::proto::golem::component::ComponentId =
                    component_id.into();

                Box::pin(
                    worker_executor_client.get_running_workers_metadata(
                        workerexecutor::v1::GetRunningWorkersMetadataRequest {
                            component_id: Some(component_id),
                            filter: filter.clone().map(|f| f.into()),
                            auth_ctx: Some(auth_ctx.clone().into())
                        }
                    )
                )
            },
            |responses| {
                responses.into_iter().map(|response| {
                    match response.into_inner() {
                        workerexecutor::v1::GetRunningWorkersMetadataResponse {
                            result:
                            Some(workerexecutor::v1::get_running_workers_metadata_response::Result::Success(workerexecutor::v1::GetRunningWorkersMetadataSuccessResponse {
                                                                                                                workers
                                                                                                            })),
                        } => {
                            let workers: Vec<AgentMetadataDto> = workers.into_iter().map(|w| w.try_into()).collect::<Result<Vec<_>, _>>().map_err(|_| WorkerExecutorError::unknown("Convert response error"))?;
                            Ok(workers)
                        }
                        workerexecutor::v1::GetRunningWorkersMetadataResponse {
                            result:
                            Some(workerexecutor::v1::get_running_workers_metadata_response::Result::Failure(err)),
                        } => Err(err.into()),
                        workerexecutor::v1::GetRunningWorkersMetadataResponse { .. } => {
                            Err("Empty response".into())
                        }
                    }
                }).collect::<Result<Vec<_>, ResponseMapResult>>()
            },
            WorkerServiceError::InternalCallError,
        ).await?;

        Ok(result.into_iter().flatten().collect())
    }

    async fn find_metadata_internal(
        &self,
        component_id: ComponentId,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<AgentMetadataDto>)> {
        let result = self
            .call_worker_executor(
                RandomExecutor,
                "get_workers_metadata",
                move |worker_executor_client| {
                    Box::pin(worker_executor_client.get_workers_metadata(
                        workerexecutor::v1::GetWorkersMetadataRequest {
                            component_id: Some(component_id.into()),
                            filter: filter.clone().map(|f| f.into()),
                            cursor: Some(cursor.clone().into()),
                            count,
                            precise,
                            environment_id: Some(environment_id.into()),
                            auth_ctx: Some(auth_ctx.clone().into()),
                        },
                    ))
                },
                |response| match response.into_inner() {
                    workerexecutor::v1::GetWorkersMetadataResponse {
                        result:
                            Some(workerexecutor::v1::get_workers_metadata_response::Result::Success(
                                workerexecutor::v1::GetWorkersMetadataSuccessResponse {
                                    workers,
                                    cursor,
                                },
                            )),
                    } => {
                        let workers = workers
                            .into_iter()
                            .map(|w| w.try_into())
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|err| {
                                WorkerExecutorError::unknown(format!(
                                    "Unexpected worker metadata in response: {err}"
                                ))
                            })?;
                        Ok((cursor.map(|c| c.into()), workers))
                    }
                    workerexecutor::v1::GetWorkersMetadataResponse {
                        result:
                            Some(workerexecutor::v1::get_workers_metadata_response::Result::Failure(
                                err,
                            )),
                    } => Err(err.into()),
                    workerexecutor::v1::GetWorkersMetadataResponse { .. } => {
                        Err("Empty response".into())
                    }
                },
                WorkerServiceError::InternalCallError,
            )
            .await?;

        Ok(result)
    }
}

impl HasRoutingTableService for WorkerExecutorWorkerClient {
    fn routing_table_service(&self) -> &Arc<RoutingTableService> {
        &self.routing_table_service
    }
}

impl HasWorkerExecutorClients for WorkerExecutorWorkerClient {
    fn worker_executor_clients(
        &self,
    ) -> &MultiTargetGrpcClient<WorkerExecutorClient<OtelGrpcService<Channel>>> {
        &self.worker_executor_clients
    }

    fn worker_executor_retry_config(&self) -> &RetryConfig {
        &self.worker_executor_retries
    }
}

#[async_trait]
impl WorkerClient for WorkerExecutorWorkerClient {
    async fn create(
        &self,
        agent_id: &AgentId,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        account_id: AccountId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
        invocation_context: Option<InvocationContext>,
        principal: Option<golem_api_grpc::proto::golem::component::Principal>,
    ) -> WorkerResult<(AgentId, AgentFingerprint)> {
        let agent_id_clone = agent_id.clone();
        let account_id_clone = account_id;
        let fingerprint = self
            .call_worker_executor(
                agent_id.clone(),
                "create_worker",
                move |worker_executor_client| {
                    let agent_id = agent_id_clone.clone();
                    Box::pin(
                        worker_executor_client.create_worker(CreateWorkerRequest {
                            agent_id: Some(agent_id.into()),
                            env: environment_variables.clone(),
                            config: config
                                .clone()
                                .into_iter()
                                .map(
                                    golem_api_grpc::proto::golem::worker::AgentConfigEntryDto::from,
                                )
                                .collect(),
                            component_owner_account_id: Some(account_id_clone.into()),
                            environment_id: Some(environment_id.into()),
                            ignore_already_existing,
                            auth_ctx: Some(auth_ctx.clone().into()),
                            principal: principal.clone(),
                            invocation_context: invocation_context.clone(),
                        }),
                    )
                },
                |response| match response.into_inner() {
                    workerexecutor::v1::CreateWorkerResponse {
                        result:
                            Some(workerexecutor::v1::create_worker_response::Result::Success(
                                workerexecutor::v1::CreateWorkerSuccessResponse {
                                    instance_id: Some(u),
                                },
                            )),
                    } => Ok(AgentFingerprint(u.into())),
                    workerexecutor::v1::CreateWorkerResponse {
                        result:
                            Some(workerexecutor::v1::create_worker_response::Result::Failure(err)),
                    } => Err(err.into()),
                    workerexecutor::v1::CreateWorkerResponse { .. } => Err("Empty response".into()),
                },
                WorkerServiceError::InternalCallError,
            )
            .await?;

        Ok((agent_id.clone(), fingerprint))
    }

    async fn connect(
        &self,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<WorkerStream<LogEvent>> {
        let agent_id_clone = agent_id.clone();
        let account_id_clone = account_id;
        let agent_id_err = agent_id.clone();
        let stream = self
            .call_worker_executor(
                agent_id.clone(),
                "connect_worker",
                move |worker_executor_client| {
                    Box::pin(worker_executor_client.connect_worker(ConnectWorkerRequest {
                        agent_id: Some(agent_id_clone.clone().into()),
                        component_owner_account_id: Some(account_id_clone.into()),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                        principal: None,
                    }))
                },
                |response| Ok(WorkerStream::new(response.into_inner())),
                |error| match error {
                    CallWorkerExecutorError::FailedToConnectToPod(status)
                        if status.code() == Code::NotFound =>
                    {
                        WorkerServiceError::AgentNotFound(agent_id_err.clone())
                    }
                    _ => WorkerServiceError::InternalCallError(error),
                },
            )
            .await?;

        Ok(stream)
    }

    async fn delete(
        &self,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let agent_id_clone = agent_id.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "delete_worker",
            move |worker_executor_client| {
                Box::pin(worker_executor_client.delete_worker(
                    workerexecutor::v1::DeleteWorkerRequest {
                        agent_id: Some(golem_api_grpc::proto::golem::worker::AgentId::from(
                            agent_id_clone.clone(),
                        )),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                        principal: None,
                    },
                ))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::DeleteWorkerResponse {
                    result: Some(workerexecutor::v1::delete_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::DeleteWorkerResponse {
                    result: Some(workerexecutor::v1::delete_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::DeleteWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;

        Ok(())
    }

    async fn complete_promise(
        &self,
        agent_id: &AgentId,
        oplog_id: u64,
        data: Vec<u8>,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let promise_id = PromiseId {
            agent_id: agent_id.clone(),
            oplog_idx: OplogIndex::from_u64(oplog_id),
        };

        let result = self
            .call_worker_executor(
                agent_id.clone(),
                "complete_promise",
                move |worker_executor_client| {
                    let promise_id = promise_id.clone();
                    let data = data.clone();
                    Box::pin(
                        worker_executor_client
                            .complete_promise(CompletePromiseRequest {
                                promise_id: Some(promise_id.into()),
                                data,
                                environment_id: Some(environment_id.into()),
                                auth_ctx: Some(auth_ctx.clone().into())
                            })
                    )
                },
                |response| {
                    match response.into_inner() {
                        workerexecutor::v1::CompletePromiseResponse {
                            result:
                            Some(workerexecutor::v1::complete_promise_response::Result::Success(
                                     success,
                                 )),
                        } => Ok(success.completed),
                        workerexecutor::v1::CompletePromiseResponse {
                            result:
                            Some(workerexecutor::v1::complete_promise_response::Result::Failure(
                                     err,
                                 )),
                        } => Err(err.into()),
                        workerexecutor::v1::CompletePromiseResponse { .. } => {
                            Err("Empty response".into())
                        }
                    }
                },
                WorkerServiceError::InternalCallError,
            )
            .await?;
        Ok(result)
    }

    async fn interrupt(
        &self,
        agent_id: &AgentId,
        recover_immediately: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let agent_id = agent_id.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "interrupt_worker",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                Box::pin(
                    worker_executor_client.interrupt_worker(InterruptWorkerRequest {
                        agent_id: Some(agent_id.into()),
                        recover_immediately,
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                        principal: None,
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::InterruptWorkerResponse {
                    result: Some(workerexecutor::v1::interrupt_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::InterruptWorkerResponse {
                    result:
                        Some(workerexecutor::v1::interrupt_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::InterruptWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;

        Ok(())
    }

    async fn get_metadata(
        &self,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<AgentMetadataDto> {
        let agent_id = agent_id.clone();
        let metadata = self.call_worker_executor(
            agent_id.clone(),
            "get_metadata",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                Box::pin(worker_executor_client.get_agent_metadata(
                    workerexecutor::v1::GetAgentMetadataRequest {
                        agent_id: Some(golem_api_grpc::proto::golem::worker::AgentId::from(agent_id.clone())),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into())
                    }
                ))
            },
            |response| {
                match response.into_inner() {
                    workerexecutor::v1::GetAgentMetadataResponse {
                        result:
                        Some(workerexecutor::v1::get_agent_metadata_response::Result::Success(metadata)),
                    } => {
                        Ok(metadata.try_into().unwrap())
                    }
                    workerexecutor::v1::GetAgentMetadataResponse {
                        result:
                        Some(workerexecutor::v1::get_agent_metadata_response::Result::Failure(err)),
                    } => {
                        Err(err.into())
                    }
                    workerexecutor::v1::GetAgentMetadataResponse { .. } => {
                        Err("Empty response".into())
                    }
                }
            },
            WorkerServiceError::InternalCallError,
        ).await?;

        Ok(metadata)
    }

    async fn find_metadata(
        &self,
        component_id: ComponentId,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<AgentMetadataDto>)> {
        if filter.as_ref().is_some_and(is_filter_with_running_status) {
            let result = self
                .find_running_metadata_internal(component_id, filter, auth_ctx)
                .await?;

            Ok((None, result.into_iter().take(count as usize).collect()))
        } else {
            self.find_metadata_internal(
                component_id,
                filter,
                cursor,
                count,
                precise,
                environment_id,
                auth_ctx,
            )
            .await
        }
    }

    async fn resume(
        &self,
        agent_id: &AgentId,
        force: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let agent_id = agent_id.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "resume_worker",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                Box::pin(worker_executor_client.resume_worker(ResumeWorkerRequest {
                    agent_id: Some(agent_id.into()),
                    force: Some(force),
                    environment_id: Some(environment_id.into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                    principal: None,
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::ResumeWorkerResponse {
                    result: Some(workerexecutor::v1::resume_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::ResumeWorkerResponse {
                    result: Some(workerexecutor::v1::resume_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::ResumeWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }

    async fn update(
        &self,
        agent_id: &AgentId,
        update_mode: AgentUpdateMode,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let agent_id = agent_id.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "update_worker",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                Box::pin(worker_executor_client.update_worker(UpdateWorkerRequest {
                    agent_id: Some(agent_id.into()),
                    mode: golem_api_grpc::proto::golem::worker::UpdateMode::from(update_mode)
                        as i32,
                    target_revision: target_revision.into(),
                    environment_id: Some(environment_id.into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                    disable_wakeup,
                    principal: None,
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::UpdateWorkerResponse {
                    result: Some(workerexecutor::v1::update_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::UpdateWorkerResponse {
                    result: Some(workerexecutor::v1::update_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::UpdateWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }

    async fn get_oplog(
        &self,
        agent_id: &AgentId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let agent_id = agent_id.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "get_oplog",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                Box::pin(
                    worker_executor_client.get_oplog(workerexecutor::v1::GetOplogRequest {
                        agent_id: Some(agent_id.into()),
                        from_oplog_index: from_oplog_index.into(),
                        cursor: cursor.clone().map(|c| c.into()),
                        count,
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::GetOplogResponse {
                    result:
                        Some(workerexecutor::v1::get_oplog_response::Result::Success(
                            workerexecutor::v1::GetOplogSuccessResponse {
                                entries,
                                next,
                                first_index_in_chunk,
                                last_index,
                            },
                        )),
                } => {
                    let entries: Vec<PublicOplogEntry> = entries
                        .into_iter()
                        .map(|e| e.try_into())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|err| {
                            WorkerExecutorError::unknown(format!(
                                "Unexpected oplog entries in error: {err}"
                            ))
                        })?;
                    Ok(GetOplogResponse {
                        entries: entries
                            .into_iter()
                            .enumerate()
                            .map(|(idx, entry)| PublicOplogEntryWithIndex {
                                oplog_index: OplogIndex::from_u64(
                                    (first_index_in_chunk) + idx as u64,
                                ),
                                entry,
                            })
                            .collect(),
                        next: next.map(|c| c.into()),
                        first_index_in_chunk,
                        last_index,
                    })
                }
                workerexecutor::v1::GetOplogResponse {
                    result: Some(workerexecutor::v1::get_oplog_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::GetOplogResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await
    }

    async fn search_oplog(
        &self,
        agent_id: &AgentId,
        cursor: Option<OplogCursor>,
        count: u64,
        query: String,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let agent_id = agent_id.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "search_oplog",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                let query_clone = query.clone();
                Box::pin(
                    worker_executor_client.search_oplog(workerexecutor::v1::SearchOplogRequest {
                        agent_id: Some(agent_id.into()),
                        query: query_clone,
                        cursor: cursor.clone().map(|c| c.into()),
                        count,
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into())
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::SearchOplogResponse {
                    result:
                    Some(golem_api_grpc::proto::golem::workerexecutor::v1::search_oplog_response::Result::Success(
                             workerexecutor::v1::SearchOplogSuccessResponse {
                                 entries,
                                 next,
                                 last_index,
                             },
                         )),
                } => {
                    let entries: Vec<PublicOplogEntryWithIndex> = entries
                        .into_iter()
                        .map(|e| e.try_into())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|err| WorkerExecutorError::unknown(format!("Unexpected oplog entries in error: {err}")))?;
                    let first_index_in_chunk = entries.first().map(|entry| entry.oplog_index).unwrap_or(OplogIndex::INITIAL).into();
                    Ok(GetOplogResponse {
                        entries,
                        next: next.map(|c| c.into()),
                        first_index_in_chunk,
                        last_index,
                    })
                }
                SearchOplogResponse {
                    result: Some(workerexecutor::v1::search_oplog_response::Result::Failure(err)),
                } => Err(err.into()),
                SearchOplogResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
            .await
    }

    async fn get_file_system_node(
        &self,
        agent_id: &AgentId,
        path: CanonicalFilePath,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
        let agent_id = agent_id.clone();
        let path_clone = path.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "get_file_system_node",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                Box::pin(
                    worker_executor_client.get_file_system_node(workerexecutor::v1::GetFileSystemNodeRequest {
                        agent_id: Some(agent_id.into()),
                        component_owner_account_id: Some(account_id.into()),
                        path: path_clone.to_string(),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                        principal: None,
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::GetFileSystemNodeResponse {
                    result: Some(golem_api_grpc::proto::golem::workerexecutor::v1::get_file_system_node_response::Result::DirSuccess(success)),
                } => {
                    success.nodes
                        .into_iter()
                        .map(|v|
                            v
                                .try_into()
                                .map_err(|_| "Failed to convert node".into())
                        )
                        .collect::<Result<Vec<_>, _>>()
                }
                workerexecutor::v1::GetFileSystemNodeResponse {
                    result: Some(workerexecutor::v1::get_file_system_node_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::GetFileSystemNodeResponse {
                    result: Some(workerexecutor::v1::get_file_system_node_response::Result::NotFound(_)),
                } => Err(WorkerServiceError::FileNotFound(path.clone()).into()),
                workerexecutor::v1::GetFileSystemNodeResponse {
                    result: Some(workerexecutor::v1::get_file_system_node_response::Result::FileSuccess(file_response)),
                } => {
                    let file_node = file_response.file
                        .ok_or(WorkerServiceError::Internal("Missing file data in response".to_string()))?
                        .try_into()
                        .map_err(|_| WorkerServiceError::Internal("Failed to convert file node".to_string()))?;
                    Ok(vec![file_node])
                },
                workerexecutor::v1::GetFileSystemNodeResponse {
                    result: None
                } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
            .await
    }

    async fn get_agent_wallet(
        &self,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<StoredCard>> {
        let agent_id = agent_id.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "get_agent_wallet",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                Box::pin(worker_executor_client.get_agent_wallet(
                    workerexecutor::v1::GetAgentWalletRequest {
                        agent_id: Some(agent_id.into()),
                        component_owner_account_id: Some(account_id.into()),
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                        principal: None,
                    },
                ))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::GetAgentWalletResponse {
                    result:
                        Some(workerexecutor::v1::get_agent_wallet_response::Result::Success(
                            success,
                        )),
                } => success
                    .wallet_cards
                    .into_iter()
                    .map(|bytes| {
                        golem_common::serialization::deserialize(&bytes).map_err(|e| {
                            WorkerServiceError::Internal(format!(
                                "Failed to decode wallet card: {e}"
                            ))
                            .into()
                        })
                    })
                    .collect::<Result<Vec<_>, _>>(),
                workerexecutor::v1::GetAgentWalletResponse {
                    result: Some(workerexecutor::v1::get_agent_wallet_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::GetAgentWalletResponse { result: None } => {
                    Err("Empty response".into())
                }
            },
            WorkerServiceError::InternalCallError,
        )
        .await
    }

    async fn get_file_contents(
        &self,
        agent_id: &AgentId,
        path: CanonicalFilePath,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>> {
        let agent_id = agent_id.clone();
        let path_clone = path.clone();
        let stream = self
            .call_worker_executor(
                agent_id.clone(),
                "read_file",
                move |worker_executor_client| {
                    Box::pin(worker_executor_client.get_file_contents(
                        workerexecutor::v1::GetFileContentsRequest {
                            agent_id: Some(agent_id.clone().into()),
                            component_owner_account_id: Some(account_id.into()),
                            file_path: path_clone.to_string(),
                            environment_id: Some(environment_id.into()),
                            auth_ctx: Some(auth_ctx.clone().into()),
                            principal: None,
                        },
                    ))
                },
                |response| Ok(WorkerStream::new(response.into_inner())),
                WorkerServiceError::InternalCallError,
            )
            .await?;

        let (header, stream) = stream.into_future().await;

        let header = header.ok_or(WorkerServiceError::Internal("Empty stream".to_string()))?;

        match header
            .map_err(|_| WorkerServiceError::Internal("Stream error".to_string()))?
            .result
        {
            Some(workerexecutor::v1::get_file_contents_response::Result::Success(_)) => Err(
                WorkerServiceError::Internal("Protocal violation".to_string()),
            ),
            Some(workerexecutor::v1::get_file_contents_response::Result::Failure(err)) => {
                let converted = WorkerExecutorError::try_from(err).map_err(|err| {
                    WorkerServiceError::Internal(format!("Failed converting errors {err}"))
                })?;
                Err(converted.into())
            }
            Some(workerexecutor::v1::get_file_contents_response::Result::Header(header)) => {
                match header.result {
                    Some(
                        workerexecutor::v1::get_file_contents_response_header::Result::Success(_),
                    ) => Ok(()),
                    Some(
                        workerexecutor::v1::get_file_contents_response_header::Result::NotAFile(_),
                    ) => Err(WorkerServiceError::BadFileType(path)),
                    Some(
                        workerexecutor::v1::get_file_contents_response_header::Result::NotFound(_),
                    ) => Err(WorkerServiceError::FileNotFound(path)),
                    None => Err(WorkerServiceError::Internal("Empty response".to_string())),
                }
            }
            None => Err(WorkerServiceError::Internal("Empty response".to_string())),
        }?;

        let stream = stream
            .map_err(|_| WorkerServiceError::Internal("Stream error".to_string()))
            .map(|item| {
                item.and_then(|response| {
                    response
                        .result
                        .ok_or(WorkerServiceError::Internal("Malformed chunk".to_string()))
                })
            })
            .map_ok(|chunk| match chunk {
                workerexecutor::v1::get_file_contents_response::Result::Success(bytes) => {
                    Ok(Bytes::from(bytes))
                }
                workerexecutor::v1::get_file_contents_response::Result::Failure(err) => {
                    let converted = WorkerExecutorError::try_from(err)
                        .map_err(|err| {
                            WorkerServiceError::Internal(format!("Failed converting errors {err}"))
                        })?
                        .into();
                    Err(converted)
                }
                workerexecutor::v1::get_file_contents_response::Result::Header(_) => Err(
                    WorkerServiceError::Internal("Unexpected header".to_string()),
                ),
            })
            .map(|item| item.and_then(|inner| inner));

        Ok(Box::pin(stream))
    }

    async fn activate_plugin(
        &self,
        agent_id: &AgentId,
        plugin_priority: PluginPriority,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let agent_id = agent_id.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "activate_plugin",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                Box::pin(
                    worker_executor_client.activate_plugin(ActivatePluginRequest {
                        agent_id: Some(agent_id.into()),
                        plugin_priority: plugin_priority.0,
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                        principal: None,
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::ActivatePluginResponse {
                    result: Some(workerexecutor::v1::activate_plugin_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::ActivatePluginResponse {
                    result:
                    Some(workerexecutor::v1::activate_plugin_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::ActivatePluginResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
            .await?;

        Ok(())
    }

    async fn deactivate_plugin(
        &self,
        agent_id: &AgentId,
        plugin_priority: PluginPriority,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let agent_id = agent_id.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "deactivate_plugin",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                Box::pin(
                    worker_executor_client.deactivate_plugin(DeactivatePluginRequest {
                        agent_id: Some(agent_id.into()),
                        plugin_priority: plugin_priority.0,
                        environment_id: Some(environment_id.into()),
                        auth_ctx: Some(auth_ctx.clone().into()),
                        principal: None,
                    }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::DeactivatePluginResponse {
                    result: Some(workerexecutor::v1::deactivate_plugin_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::DeactivatePluginResponse {
                    result:
                    Some(workerexecutor::v1::deactivate_plugin_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::DeactivatePluginResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
            .await?;

        Ok(())
    }

    async fn fork_worker(
        &self,
        source_agent_id: &AgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        environment_id: EnvironmentId,
        account_id: AccountId,
        account_email: AccountEmail,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let source_agent_id = source_agent_id.clone();
        let target_agent_id = target_agent_id.clone();
        self.call_worker_executor(
            source_agent_id.clone(),
            "fork_worker",
            move |worker_executor_client| {
                let source_agent_id = source_agent_id.clone();
                let target_agent_id = target_agent_id.clone();
                Box::pin(worker_executor_client.fork_worker(ForkWorkerRequest {
                    source_agent_id: Some(source_agent_id.into()),
                    target_agent_id: Some(target_agent_id.into()),
                    component_owner_account_id: Some(account_id.into()),
                    component_owner_account_email: account_email.as_str().to_string(),
                    oplog_index_cutoff: oplog_index_cut_off.into(),
                    environment_id: Some(environment_id.into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                    principal: None,
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::ForkWorkerResponse {
                    result: Some(workerexecutor::v1::fork_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::ForkWorkerResponse {
                    result: Some(workerexecutor::v1::fork_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::ForkWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }

    async fn revert_worker(
        &self,
        agent_id: &AgentId,
        target: RevertWorkerTarget,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let agent_id = agent_id.clone();
        self.call_worker_executor(
            agent_id.clone(),
            "revert_worker",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                let target = target.clone();
                Box::pin(worker_executor_client.revert_worker(RevertWorkerRequest {
                    agent_id: Some(agent_id.into()),
                    target: Some(target.into()),
                    environment_id: Some(environment_id.into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                    principal: None,
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::RevertWorkerResponse {
                    result: Some(workerexecutor::v1::revert_worker_response::Result::Success(_)),
                } => Ok(()),
                workerexecutor::v1::RevertWorkerResponse {
                    result: Some(workerexecutor::v1::revert_worker_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::RevertWorkerResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }

    async fn cancel_invocation(
        &self,
        agent_id: &AgentId,
        idempotency_key: &IdempotencyKey,
        environment_id: EnvironmentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let agent_id = agent_id.clone();
        let idempotency_key = idempotency_key.clone();
        let canceled = self.call_worker_executor(
            agent_id.clone(),
            "cancel_invocation",
            move |worker_executor_client| {
                let agent_id = agent_id.clone();
                let idempotency_key = idempotency_key.clone();
                Box::pin(worker_executor_client.cancel_invocation(CancelInvocationRequest {
                    agent_id: Some(agent_id.into()),
                    idempotency_key: Some(idempotency_key.into()),
                    environment_id: Some(environment_id.into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                    principal: None,
                }))
            },
            |response| match response.into_inner() {
                workerexecutor::v1::CancelInvocationResponse {
                    result: Some(workerexecutor::v1::cancel_invocation_response::Result::Success(canceled)),
                } => Ok(canceled),
                workerexecutor::v1::CancelInvocationResponse {
                    result: Some(workerexecutor::v1::cancel_invocation_response::Result::Failure(err)),
                } => Err(err.into()),
                workerexecutor::v1::CancelInvocationResponse { .. } => Err("Empty response".into()),
            },
            WorkerServiceError::InternalCallError,
        )
            .await?;
        Ok(canceled)
    }

    async fn invoke_agent(
        &self,
        agent_id: &AgentId,
        method_name: Option<String>,
        method_parameters: Option<golem_api_grpc::proto::golem::schema::SchemaValue>,
        mode: i32,
        schedule_at: Option<::prost_types::Timestamp>,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: Option<InvocationContext>,
        freshness_disposition: InvocationFreshnessDisposition,
        config: Vec<AgentConfigEntryDto>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
        principal: golem_api_grpc::proto::golem::component::Principal,
    ) -> WorkerResult<AgentInvocationOutput> {
        let agent_id = agent_id.clone();
        let agent_id_clone = agent_id.clone();
        let first_dispatch = Arc::new(AtomicBool::new(true));

        let result = self
            .call_worker_executor(
                agent_id.clone(),
                "invoke_agent_session",
                move |worker_executor_client| {
                    let freshness_disposition =
                        freshness_disposition_for_dispatch(freshness_disposition, &first_dispatch);
                    let start = InvocationStart {
                        agent_id: Some(agent_id_clone.clone().into()),
                        method_name: method_name.clone(),
                        input: method_parameters.clone(),
                        idempotency_key: idempotency_key.clone().map(Into::into),
                        context: invocation_context.clone(),
                        auth_ctx: Some(auth_ctx.clone().into()),
                        principal: Some(principal.clone()),
                        environment_id: Some(environment_id.into()),
                        config: config.clone().into_iter().map(Into::into).collect(),
                        component_owner_account_id: Some(account_id.into()),
                        mode,
                        schedule_at,
                        freshness_disposition: match freshness_disposition {
                            InvocationFreshnessDisposition::MayExist => {
                                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                                    as i32
                            }
                            InvocationFreshnessDisposition::KnownFresh => {
                                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh
                                    as i32
                            }
                        },
                    };
                    Box::pin(run_one_shot_invocation_session(
                        worker_executor_client,
                        start,
                    ))
                },
                |outcome| match outcome {
                    OneShotInvocationSessionResult::Success(output) => Ok(output),
                    OneShotInvocationSessionResult::Rejected(rejected) => {
                        Err(decode_invocation_rejection(rejected).into())
                    }
                    OneShotInvocationSessionResult::Failure(failure) => {
                        Err(decode_invocation_failure(failure).into())
                    }
                    OneShotInvocationSessionResult::ProtocolFailure(details) => {
                        Err(WorkerExecutorError::invalid_request(details).into())
                    }
                },
                WorkerServiceError::InternalCallError,
            )
            .await?;

        Ok(result)
    }

    async fn invoke_agent_session(
        &self,
        agent_id: &AgentId,
        request: InvocationRequestStream,
    ) -> WorkerResult<InvocationResponseStream> {
        let routing_table = self
            .routing_table_service
            .get_routing_table()
            .await
            .map_err(|error| {
                WorkerServiceError::InternalCallError(
                    CallWorkerExecutorError::FailedToGetRoutingTable(error),
                )
            })?;
        let pod = routing_table.lookup(agent_id).ok_or_else(|| {
            WorkerServiceError::InternalCallError(CallWorkerExecutorError::FailedToConnectToPod(
                Status::unavailable(format!("no active shard for agent {agent_id}")),
            ))
        })?;
        let request = Arc::new(std::sync::Mutex::new(Some(request)));
        let response = self
            .worker_executor_clients
            .call_without_retry(
                "invoke_agent_session",
                pod.uri(self.worker_executor_clients.uses_tls()),
                move |worker_executor_client| {
                    let request = request
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .take();
                    invoke_agent_session_once(worker_executor_client, request)
                },
            )
            .await
            .map_err(|status| {
                WorkerServiceError::InternalCallError(
                    CallWorkerExecutorError::FailedToConnectToPod(status),
                )
            })?;
        Ok(Box::pin(response.into_inner()))
    }

    async fn process_oplog_entries(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        component_revision: ComponentRevision,
        idempotency_key: IdempotencyKey,
        account_id: AccountId,
        config: std::collections::HashMap<String, String>,
        metadata: golem_api_grpc::proto::golem::worker::AgentMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let target_agent_id = target_agent_id.clone();
        self.call_worker_executor(
            target_agent_id.clone(),
            "process_oplog_entries",
            move |worker_executor_client| {
                let target_agent_id = target_agent_id.clone();
                Box::pin(
                    worker_executor_client
                        .process_oplog_entries(ProcessOplogEntriesRequest {
                            agent_id: Some(target_agent_id.into()),
                            environment_id: Some(environment_id.into()),
                            component_revision: component_revision.into(),
                            idempotency_key: Some(idempotency_key.clone().into()),
                            account_id: Some(account_id.into()),
                            config: config.clone(),
                            metadata: Some(metadata.clone()),
                            first_entry_index: first_entry_index.into(),
                            entries: entries.clone(),
                            auth_ctx: Some(auth_ctx.clone().into()),
                        }),
                )
            },
            |response| match response.into_inner() {
                workerexecutor::v1::ProcessOplogEntriesResponse {
                    result:
                        Some(
                            workerexecutor::v1::process_oplog_entries_response::Result::Success(_),
                        ),
                } => Ok(()),
                workerexecutor::v1::ProcessOplogEntriesResponse {
                    result:
                        Some(
                            workerexecutor::v1::process_oplog_entries_response::Result::Failure(err),
                        ),
                } => Err(err.into()),
                workerexecutor::v1::ProcessOplogEntriesResponse { .. } => {
                    Err("Empty response".into())
                }
            },
            WorkerServiceError::InternalCallError,
        )
        .await?;
        Ok(())
    }
}

fn is_filter_with_running_status(filter: &AgentFilter) -> bool {
    match filter {
        AgentFilter::Status(f)
            if f.value == AgentStatus::Running && f.comparator == FilterComparator::Equal =>
        {
            true
        }
        AgentFilter::And(f) => f.filters.iter().any(is_filter_with_running_status),
        _ => false,
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::freshness_disposition_for_dispatch;
    use golem_common::model::agent::InvocationFreshnessDisposition;
    use std::sync::atomic::AtomicBool;
    use test_r::test;

    #[test]
    fn known_fresh_is_only_used_for_the_first_dispatch() {
        let first_dispatch = AtomicBool::new(true);

        assert_eq!(
            freshness_disposition_for_dispatch(
                InvocationFreshnessDisposition::KnownFresh,
                &first_dispatch,
            ),
            InvocationFreshnessDisposition::KnownFresh
        );
        assert_eq!(
            freshness_disposition_for_dispatch(
                InvocationFreshnessDisposition::KnownFresh,
                &first_dispatch,
            ),
            InvocationFreshnessDisposition::MayExist
        );
    }

    #[test]
    fn may_exist_remains_may_exist_on_every_dispatch() {
        let first_dispatch = AtomicBool::new(true);

        assert_eq!(
            freshness_disposition_for_dispatch(
                InvocationFreshnessDisposition::MayExist,
                &first_dispatch,
            ),
            InvocationFreshnessDisposition::MayExist
        );
        assert_eq!(
            freshness_disposition_for_dispatch(
                InvocationFreshnessDisposition::MayExist,
                &first_dispatch,
            ),
            InvocationFreshnessDisposition::MayExist
        );
    }
}

#[cfg(test)]
mod one_shot_session_tests {
    use super::{
        OneShotInvocationSessionResult, collect_one_shot_invocation_session,
        decode_invocation_failure,
    };
    use futures::stream;
    use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
    use golem_api_grpc::proto::golem::common::Empty;
    use golem_api_grpc::proto::golem::schema::{SchemaValue, schema_value};
    use golem_api_grpc::proto::golem::worker::{
        AgentId, IdempotencyKey, InvocationAccepted, InvocationFailure, InvocationFailureKind,
        InvocationRequest, InvocationResponse, InvocationSessionCompletion,
        InvocationSessionResult, InvocationStart, invocation_request, invocation_response,
        invocation_session_completion, invocation_session_result,
    };
    use golem_common::model::AgentFingerprint;
    use golem_common::model::oplog::{AgentError, OplogIndex};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use test_r::test;
    use tonic::Status;

    fn key() -> Option<IdempotencyKey> {
        Some(IdempotencyKey {
            value: "session-key".to_string(),
        })
    }

    fn agent_id() -> Option<AgentId> {
        Some(
            golem_common::model::AgentId {
                component_id: golem_common::model::component::ComponentId(uuid::Uuid::nil()),
                agent_id: "agent".to_string(),
            }
            .into(),
        )
    }

    fn state_after_start() -> InvocationSessionState {
        let mut state = InvocationSessionState::default();
        state
            .validate_trusted_request(&InvocationRequest {
                request: Some(invocation_request::Request::Start(InvocationStart {
                    input: Some(SchemaValue {
                        value: Some(schema_value::Value::U8Value(1)),
                    }),
                    idempotency_key: key(),
                    ..Default::default()
                })),
            })
            .unwrap();
        state
    }

    fn frame(response: invocation_response::Response) -> Result<InvocationResponse, Status> {
        Ok(InvocationResponse {
            response: Some(response),
        })
    }

    fn accepted() -> invocation_response::Response {
        invocation_response::Response::Accepted(InvocationAccepted {
            agent_id: agent_id(),
            idempotency_key: key(),
            component_revision: Some(3),
        })
    }

    fn no_result() -> invocation_response::Response {
        invocation_response::Response::Result(InvocationSessionResult {
            result: Some(invocation_session_result::Result::NoResult(Empty {})),
            component_revision: Some(3),
            agent_id: agent_id(),
            idempotency_key: key(),
            ..Default::default()
        })
    }

    fn finished(outcome: invocation_session_completion::Outcome) -> invocation_response::Response {
        invocation_response::Response::Finished(InvocationSessionCompletion {
            outcome: Some(outcome),
        })
    }

    #[test]
    async fn successful_session_preserves_unary_result_metadata() {
        let responses = stream::iter([
            frame(accepted()),
            frame(invocation_response::Response::Result(
                InvocationSessionResult {
                    result: Some(invocation_session_result::Result::NoResult(Empty {})),
                    component_revision: Some(3),
                    agent_id: agent_id(),
                    idempotency_key: key(),
                    fuel_consumed: Some(17),
                    status: Some(
                        golem_api_grpc::proto::golem::worker::InvocationStatus::Complete as i32,
                    ),
                    oplog_index: Some(29),
                    agent_fingerprint: Some(uuid::Uuid::nil().into()),
                },
            )),
            frame(finished(invocation_session_completion::Outcome::Success(
                Empty {},
            ))),
        ]);

        let result = collect_one_shot_invocation_session(responses, state_after_start())
            .await
            .unwrap();
        let OneShotInvocationSessionResult::Success(output) = result else {
            panic!("expected a successful invocation session");
        };
        assert_eq!(output.agent_id.unwrap().agent_id, "agent");
        assert_eq!(output.idempotency_key.unwrap().value, "session-key");
        assert_eq!(output.component_revision.unwrap().get(), 3);
        assert_eq!(output.consumed_fuel, Some(17));
        assert_eq!(
            output.invocation_status,
            Some(golem_common::model::InvocationStatus::Complete)
        );
        assert_eq!(output.oplog_index, Some(OplogIndex::from_u64(29)));
        assert_eq!(
            output.agent_fingerprint,
            Some(AgentFingerprint(uuid::Uuid::nil()))
        );
    }

    #[test]
    async fn typed_executor_failure_survives_the_session_adapter() {
        let expected = WorkerExecutorError::InvocationFailed {
            error: AgentError::Unknown("failed".to_string()),
            stderr: "guest stderr".to_string(),
        };
        let responses = stream::iter([
            frame(accepted()),
            frame(finished(invocation_session_completion::Outcome::Failure(
                InvocationFailure {
                    kind: InvocationFailureKind::Execution as i32,
                    code: "execution".to_string(),
                    message: "failed".to_string(),
                    worker_error: Some(expected.clone().into()),
                },
            ))),
        ]);

        let result = collect_one_shot_invocation_session(responses, state_after_start())
            .await
            .unwrap();
        let OneShotInvocationSessionResult::Failure(failure) = result else {
            panic!("expected an invocation failure");
        };
        assert_eq!(decode_invocation_failure(failure), expected);
    }

    #[test]
    async fn successful_completion_before_result_is_a_protocol_failure() {
        let responses = stream::iter([
            frame(accepted()),
            frame(finished(invocation_session_completion::Outcome::Success(
                Empty {},
            ))),
        ]);

        let result = collect_one_shot_invocation_session(responses, state_after_start())
            .await
            .unwrap();
        assert!(matches!(
            result,
            OneShotInvocationSessionResult::ProtocolFailure(details)
                if details.contains("before publishing a result")
        ));
    }

    #[test]
    async fn session_is_drained_and_rejects_frames_after_completion() {
        let responses = stream::iter([
            frame(accepted()),
            frame(no_result()),
            frame(finished(invocation_session_completion::Outcome::Success(
                Empty {},
            ))),
            frame(no_result()),
        ]);

        let result = collect_one_shot_invocation_session(responses, state_after_start())
            .await
            .unwrap();
        assert!(matches!(
            result,
            OneShotInvocationSessionResult::ProtocolFailure(details)
                if details.contains("after completion")
        ));
    }

    #[test]
    async fn response_transport_error_after_completion_is_retriable() {
        let responses = stream::iter([
            frame(accepted()),
            frame(no_result()),
            frame(finished(invocation_session_completion::Outcome::Success(
                Empty {},
            ))),
            Err(Status::unavailable("response did not close cleanly")),
        ]);

        let error = collect_one_shot_invocation_session(responses, state_after_start())
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unavailable);
    }

    #[test]
    async fn typed_transport_failure_is_retriable() {
        let responses = stream::iter([
            frame(accepted()),
            frame(finished(invocation_session_completion::Outcome::Failure(
                InvocationFailure {
                    kind: InvocationFailureKind::Transport as i32,
                    code: "transport".to_string(),
                    message: "request transport failed".to_string(),
                    worker_error: None,
                },
            ))),
        ]);

        let error = collect_one_shot_invocation_session(responses, state_after_start())
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::Unavailable);
        assert!(error.message().contains("request transport failed"));
    }
}

#[cfg(test)]
mod rejection_mapping_tests {
    use super::{WorkerClient, WorkerExecutorWorkerClient, decode_invocation_rejection};
    use futures::{Stream, stream};
    use golem_api_grpc::proto::golem::schema::{SchemaValue, schema_value};
    use golem_api_grpc::proto::golem::shardmanager::{
        IpAddress, Pod as GrpcPod, RoutingTable as GrpcRoutingTable, RoutingTableEntry, ShardId,
        ip_address,
    };
    use golem_api_grpc::proto::golem::worker::v1::{AgentError, agent_error};
    use golem_api_grpc::proto::golem::worker::{
        InvocationRejected, InvocationRejectionReason, InvocationRequest, InvocationResponse,
        invocation_response,
    };
    use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_server::{
        WorkerExecutor, WorkerExecutorServer,
    };
    use golem_api_grpc::proto::golem::workerexecutor::v1::*;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::{InvocationFreshnessDisposition, Principal};
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::quota::{ResourceDefinitionId, ResourceName};
    use golem_common::model::{AgentId, RetryConfig, RoutingTable};
    use golem_service_base::clients::shard_manager::{
        BatchRenewalEntry, QuotaError, ShardManager, ShardManagerError,
    };
    use golem_service_base::grpc::client::{GrpcClientConfig, MultiTargetGrpcClient};
    use golem_service_base::model::auth::AuthCtx;
    use golem_service_base::model::quota_lease::{PendingReservation, QuotaLease};
    use golem_service_base::service::routing_table::{RoutingTableConfig, RoutingTableService};
    use std::net::Ipv4Addr;
    use std::pin::Pin;
    use std::sync::Arc;
    use test_r::test;
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::codec::CompressionEncoding;
    use tonic::{Request, Response, Status};
    use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

    fn public_error_for_rejection(reason: InvocationRejectionReason) -> agent_error::Error {
        let rejection = InvocationRejected {
            reason: reason as i32,
            error: "rejected".to_string(),
            ..Default::default()
        };
        let error: AgentError = decode_invocation_rejection(rejection).into();
        error.error.expect("missing public error")
    }

    #[test]
    fn validation_rejection_remains_a_bad_request() {
        assert!(matches!(
            public_error_for_rejection(InvocationRejectionReason::Validation),
            agent_error::Error::BadRequest(_)
        ));
    }

    #[test]
    fn unauthorized_rejection_remains_unauthorized() {
        assert!(matches!(
            public_error_for_rejection(InvocationRejectionReason::Unauthorized),
            agent_error::Error::Unauthorized(_)
        ));
    }

    #[test]
    fn internal_rejection_remains_internal() {
        assert!(matches!(
            public_error_for_rejection(InvocationRejectionReason::Internal),
            agent_error::Error::InternalError(_)
        ));
    }

    #[derive(Clone)]
    struct StaticShardManager(RoutingTable);

    #[async_trait::async_trait]
    impl ShardManager for StaticShardManager {
        async fn get_routing_table(&self) -> Result<RoutingTable, ShardManagerError> {
            Ok(self.0.clone())
        }

        async fn register(
            &self,
            _port: u16,
            _pod_name: Option<String>,
        ) -> Result<u32, ShardManagerError> {
            unreachable!()
        }

        async fn acquire_quota_lease(
            &self,
            _environment_id: EnvironmentId,
            _resource_name: ResourceName,
            _port: u16,
        ) -> Result<QuotaLease, QuotaError> {
            unreachable!()
        }

        async fn renew_quota_lease(
            &self,
            _resource_definition_id: ResourceDefinitionId,
            _port: u16,
            _epoch: u64,
            _unused: u64,
            _pending_reservations: Vec<PendingReservation>,
        ) -> Result<QuotaLease, QuotaError> {
            unreachable!()
        }

        async fn batch_renew_quota_leases(
            &self,
            _port: u16,
            _renewals: Vec<BatchRenewalEntry>,
        ) -> Result<Vec<Result<QuotaLease, QuotaError>>, ShardManagerError> {
            unreachable!()
        }

        async fn release_quota_lease(
            &self,
            _resource_definition_id: ResourceDefinitionId,
            _port: u16,
            _epoch: u64,
            _unused: u64,
        ) -> Result<(), QuotaError> {
            unreachable!()
        }
    }

    #[derive(Clone)]
    struct RejectingExecutor;

    macro_rules! unimplemented_unary {
        ($name:ident, $request:ty, $response:ty) => {
            fn $name<'life0, 'async_trait>(
                &'life0 self,
                _request: Request<$request>,
            ) -> Pin<
                Box<
                    dyn std::future::Future<Output = Result<Response<$response>, Status>>
                        + Send
                        + 'async_trait,
                >,
            >
            where
                'life0: 'async_trait,
                Self: 'async_trait,
            {
                Box::pin(async { Err(Status::unimplemented(stringify!($name))) })
            }
        };
    }

    #[tonic::async_trait]
    impl WorkerExecutor for RejectingExecutor {
        type ConnectWorkerStream = Pin<
            Box<
                dyn Stream<Item = Result<golem_api_grpc::proto::golem::worker::LogEvent, Status>>
                    + Send,
            >,
        >;
        type GetFileContentsStream =
            Pin<Box<dyn Stream<Item = Result<GetFileContentsResponse, Status>> + Send>>;
        type InvokeAgentSessionStream =
            Pin<Box<dyn Stream<Item = Result<InvocationResponse, Status>> + Send>>;

        unimplemented_unary!(create_worker, CreateWorkerRequest, CreateWorkerResponse);
        unimplemented_unary!(delete_worker, DeleteWorkerRequest, DeleteWorkerResponse);
        unimplemented_unary!(
            complete_promise,
            CompletePromiseRequest,
            CompletePromiseResponse
        );
        unimplemented_unary!(
            interrupt_worker,
            InterruptWorkerRequest,
            InterruptWorkerResponse
        );
        unimplemented_unary!(revoke_shards, RevokeShardsRequest, RevokeShardsResponse);
        unimplemented_unary!(assign_shards, AssignShardsRequest, AssignShardsResponse);
        unimplemented_unary!(
            set_shard_assignment,
            SetShardAssignmentRequest,
            SetShardAssignmentResponse
        );
        unimplemented_unary!(
            get_agent_metadata,
            GetAgentMetadataRequest,
            GetAgentMetadataResponse
        );
        unimplemented_unary!(resume_worker, ResumeWorkerRequest, ResumeWorkerResponse);
        unimplemented_unary!(
            get_running_workers_metadata,
            GetRunningWorkersMetadataRequest,
            GetRunningWorkersMetadataResponse
        );
        unimplemented_unary!(
            get_workers_metadata,
            GetWorkersMetadataRequest,
            GetWorkersMetadataResponse
        );
        unimplemented_unary!(update_worker, UpdateWorkerRequest, UpdateWorkerResponse);
        unimplemented_unary!(get_oplog, GetOplogRequest, GetOplogResponse);
        unimplemented_unary!(search_oplog, SearchOplogRequest, SearchOplogResponse);
        unimplemented_unary!(fork_worker, ForkWorkerRequest, ForkWorkerResponse);
        unimplemented_unary!(revert_worker, RevertWorkerRequest, RevertWorkerResponse);
        unimplemented_unary!(
            cancel_invocation,
            CancelInvocationRequest,
            CancelInvocationResponse
        );
        unimplemented_unary!(
            get_file_system_node,
            GetFileSystemNodeRequest,
            GetFileSystemNodeResponse
        );
        unimplemented_unary!(
            get_agent_wallet,
            GetAgentWalletRequest,
            GetAgentWalletResponse
        );
        unimplemented_unary!(
            activate_plugin,
            ActivatePluginRequest,
            ActivatePluginResponse
        );
        unimplemented_unary!(
            deactivate_plugin,
            DeactivatePluginRequest,
            DeactivatePluginResponse
        );
        unimplemented_unary!(
            process_oplog_entries,
            ProcessOplogEntriesRequest,
            ProcessOplogEntriesResponse
        );

        async fn connect_worker(
            &self,
            _request: Request<ConnectWorkerRequest>,
        ) -> Result<Response<Self::ConnectWorkerStream>, Status> {
            Err(Status::unimplemented("connect_worker"))
        }

        async fn get_file_contents(
            &self,
            _request: Request<GetFileContentsRequest>,
        ) -> Result<Response<Self::GetFileContentsStream>, Status> {
            Err(Status::unimplemented("get_file_contents"))
        }

        async fn invoke_agent_session(
            &self,
            request: Request<tonic::Streaming<InvocationRequest>>,
        ) -> Result<Response<Self::InvokeAgentSessionStream>, Status> {
            let mut requests = request.into_inner();
            let start = requests.message().await?.expect("missing invocation start");
            let (idempotency_key, agent_id) = match start.request {
                Some(golem_api_grpc::proto::golem::worker::invocation_request::Request::Start(
                    start,
                )) => (start.idempotency_key, start.agent_id),
                other => panic!("expected invocation start, got {other:?}"),
            };
            Ok(Response::new(Box::pin(stream::iter([Ok(
                InvocationResponse {
                    response: Some(invocation_response::Response::Rejected(
                        InvocationRejected {
                            reason: InvocationRejectionReason::NotFound as i32,
                            error: "agent not found".to_string(),
                            idempotency_key,
                            agent_id,
                            component_revision: None,
                        },
                    )),
                },
            )]))))
        }
    }

    #[test]
    async fn unary_not_found_rejection_preserves_the_public_error_category() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(
                    WorkerExecutorServer::new(RejectingExecutor)
                        .accept_compressed(CompressionEncoding::Gzip)
                        .send_compressed(CompressionEncoding::Gzip),
                )
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let routing_table: RoutingTable = GrpcRoutingTable {
            number_of_shards: 1,
            shard_assignments: vec![RoutingTableEntry {
                shard_id: Some(ShardId { value: 0 }),
                pod: Some(GrpcPod {
                    ip: Some(IpAddress {
                        kind: Some(ip_address::Kind::Ipv4(u32::from(Ipv4Addr::LOCALHOST))),
                    }),
                    port: port.into(),
                }),
            }],
        }
        .try_into()
        .unwrap();
        let routing = Arc::new(RoutingTableService::new(
            RoutingTableConfig::default(),
            Arc::new(StaticShardManager(routing_table)),
        ));
        let clients = MultiTargetGrpcClient::new(
            "provisional-rejecting-executor",
            |channel: OtelGrpcService<_>, max_message_size| {
                golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
                    .max_decoding_message_size(max_message_size)
                    .max_encoding_message_size(max_message_size)
            },
            GrpcClientConfig::default(),
        );
        let client = WorkerExecutorWorkerClient::new(clients, RetryConfig::default(), routing);
        let agent_id = AgentId {
            component_id: ComponentId::new(),
            agent_id: "missing".to_string(),
        };

        let error = client
            .invoke_agent(
                &agent_id,
                Some("run".to_string()),
                Some(SchemaValue {
                    value: Some(schema_value::Value::TupleValue(Default::default())),
                }),
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                None,
                Some(golem_common::model::IdempotencyKey::new(
                    "session-key".to_string(),
                )),
                None,
                InvocationFreshnessDisposition::MayExist,
                vec![],
                EnvironmentId::new(),
                AccountId::new(),
                AuthCtx::System,
                Principal::anonymous().into(),
            )
            .await
            .unwrap_err();

        let public_error: AgentError = error.into();
        assert!(
            matches!(public_error.error, Some(agent_error::Error::NotFound(_))),
            "InvocationRejected(NotFound) must remain a public not-found error, got {public_error:?}"
        );
    }
}
