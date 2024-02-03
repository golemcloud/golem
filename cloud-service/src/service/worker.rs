use core::task::{Context, Poll};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use golem_api_grpc::proto::golem::template::FunctionResult;
use golem_api_grpc::proto::golem::worker::{
    InvokeResult as ProtoInvokeResult, LogEvent, Val as ProtoVal,
};
use golem_api_grpc::proto::golem::workerexecutor;
use golem_api_grpc::proto::golem::workerexecutor::worker_executor_client::WorkerExecutorClient;
use golem_api_grpc::proto::golem::workerexecutor::{
    CompletePromiseRequest, ConnectWorkerRequest, CreateWorkerRequest, GetInvocationKeyRequest,
    InterruptWorkerRequest, InvokeAndAwaitWorkerRequest, InvokeWorkerRequest, ResumeWorkerRequest,
};
use golem_common::model::ProjectId;
use golem_common::model::{
    AccountId, CallingConvention, InvocationKey, ShardId, TemplateId, WorkerStatus,
};
use serde_json::Value;
use tokio::time::sleep;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::transport::Channel;
use tonic::{Status, Streaming};
use tracing::{debug, error, info};

use crate::auth::AccountAuthorisation;
use crate::model::*;
use crate::repo::account_connections::AccountConnectionsRepo;
use crate::repo::account_workers::AccountWorkersRepo;
use crate::service::plan_limit::{CheckLimitResult, LimitResult, PlanLimitError, PlanLimitService};
use crate::service::project_auth::{ProjectAuthorisationError, ProjectAuthorisationService};
use crate::service::template::{TemplateError, TemplateService};
use golem_service_base::model::*;
use golem_service_base::routing_table::{RoutingTableError, RoutingTableService};
use golem_service_base::typechecker::{TypeCheckIn, TypeCheckOut};
use golem_service_base::worker_executor_clients::WorkerExecutorClients;

pub struct ConnectWorkerStream {
    stream: ReceiverStream<Result<LogEvent, Status>>,
    account_connections_repository: Arc<dyn AccountConnectionsRepo + Send + Sync>,
    account_id: AccountId,
    cancel: tokio_util::sync::CancellationToken,
}

impl ConnectWorkerStream {
    pub fn new(
        streaming: Streaming<LogEvent>,
        account_connections_repository: Arc<dyn AccountConnectionsRepo + Send + Sync>,
        account_id: AccountId,
    ) -> Self {
        // Create a channel which is Send and Sync.
        // Streaming is not Sync.
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        let mut streaming = streaming;

        let cancel = tokio_util::sync::CancellationToken::new();

        tokio::spawn({
            let cancel = cancel.clone();

            let forward_loop = {
                let sender = sender.clone();
                async move {
                    while let Some(message) = streaming.next().await {
                        if let Err(error) = sender.send(message).await {
                            tracing::info!("Failed to forward WorkerStream: {error}");
                            break;
                        }
                    }
                }
            };

            async move {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!("WorkerStream cancelled");
                    }
                    _ = forward_loop => {
                        tracing::info!("WorkerStream forward loop finished");
                    }
                };
                sender.closed().await;
            }
        });

        let stream = ReceiverStream::new(receiver);

        Self {
            stream,
            account_connections_repository,
            account_id,
            cancel,
        }
    }
}

impl Stream for ConnectWorkerStream {
    type Item = Result<LogEvent, Status>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<LogEvent, Status>>> {
        self.stream.poll_next_unpin(cx)
    }
}

impl Drop for ConnectWorkerStream {
    fn drop(&mut self) {
        self.cancel.cancel();

        let account_connections_repository = self.account_connections_repository.clone();
        let account_id = self.account_id.clone();
        tokio::spawn(async move {
            decrement_account_connections(account_connections_repository, &account_id).await
        });
    }
}

pub enum WorkerError {
    Internal(String),
    TypeCheckerError(String),
    DelegatedTemplateServiceError(TemplateError),
    VersionedTemplateIdNotFound(VersionedTemplateId),
    TemplateNotFound(TemplateId),
    ProjectIdNotFound(ProjectId),
    AccountIdNotFound(AccountId),
    WorkerNotFound(WorkerId),
    Unauthorized(String),
    LimitExceeded(String),
    Golem(GolemError),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            WorkerError::Internal(ref string) => write!(f, "Internal error: {}", string),
            WorkerError::TypeCheckerError(ref string) => {
                write!(f, "Type checker error: {}", string)
            }
            WorkerError::DelegatedTemplateServiceError(ref error) => {
                write!(f, "Delegated template service error: {}", error)
            }
            WorkerError::VersionedTemplateIdNotFound(ref versioned_template_id) => write!(
                f,
                "Versioned template id not found: {}",
                versioned_template_id
            ),
            WorkerError::TemplateNotFound(ref template_id) => {
                write!(f, "Template not found: {}", template_id)
            }
            WorkerError::ProjectIdNotFound(ref project_id) => {
                write!(f, "Project id not found: {}", project_id)
            }
            WorkerError::AccountIdNotFound(ref account_id) => {
                write!(f, "Account id not found: {}", account_id)
            }
            WorkerError::WorkerNotFound(ref worker_id) => {
                write!(f, "Worker not found: {}", worker_id)
            }
            WorkerError::Unauthorized(ref string) => write!(f, "Unauthorized: {}", string),
            WorkerError::LimitExceeded(ref string) => write!(f, "Limit exceeded: {}", string),
            WorkerError::Golem(ref error) => write!(f, "Golem error: {:?}", error),
        }
    }
}

impl From<ProjectAuthorisationError> for WorkerError {
    fn from(error: ProjectAuthorisationError) -> Self {
        match error {
            ProjectAuthorisationError::Internal(error) => WorkerError::Internal(error),
            ProjectAuthorisationError::Unauthorized(error) => WorkerError::Unauthorized(error),
        }
    }
}

impl From<PlanLimitError> for WorkerError {
    fn from(error: PlanLimitError) -> Self {
        match error {
            PlanLimitError::AccountIdNotFound(account_id) => {
                WorkerError::AccountIdNotFound(account_id)
            }
            PlanLimitError::ProjectIdNotFound(project_id) => {
                WorkerError::ProjectIdNotFound(project_id)
            }
            PlanLimitError::TemplateIdNotFound(template_id) => {
                WorkerError::TemplateNotFound(template_id)
            }
            PlanLimitError::Internal(string) => WorkerError::Internal(string),
            PlanLimitError::Unauthorized(string) => WorkerError::Unauthorized(string),
        }
    }
}

impl From<RoutingTableError> for WorkerError {
    fn from(error: RoutingTableError) -> Self {
        WorkerError::Internal(format!("Unable to get routing table: {:?}", error))
    }
}

impl From<TemplateError> for WorkerError {
    fn from(error: TemplateError) -> Self {
        WorkerError::DelegatedTemplateServiceError(error)
    }
}

#[async_trait]
pub trait WorkerService {
    async fn create(
        &self,
        worker_id: &WorkerId,
        template_version: i32,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        auth: &AccountAuthorisation,
    ) -> Result<VersionedWorkerId, WorkerError>;

    async fn connect(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<ConnectWorkerStream, WorkerError>;

    async fn delete(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError>;

    async fn get_invocation_key(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<InvocationKey, WorkerError>;

    async fn invoke_and_await_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Value,
        calling_convention: &CallingConvention,
        auth: &AccountAuthorisation,
    ) -> Result<Value, WorkerError>;

    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Vec<ProtoVal>,
        calling_convention: &CallingConvention,
        auth: &AccountAuthorisation,
    ) -> Result<ProtoInvokeResult, WorkerError>;

    async fn invoke_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Value,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError>;

    async fn invoke_fn_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Vec<ProtoVal>,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError>;

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: i32,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<bool, WorkerError>;

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError>;

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::WorkerMetadata, WorkerError>;

    async fn resume(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError>;
}

pub struct WorkerServiceDefault {
    worker_executor_clients: Arc<dyn WorkerExecutorClients + Send + Sync>,
    template_service: Arc<dyn TemplateService + Send + Sync>,
    routing_table_service: Arc<dyn RoutingTableService + Send + Sync>,
    project_authorisation_service: Arc<dyn ProjectAuthorisationService + Send + Sync>,
    account_connections_repository: Arc<dyn AccountConnectionsRepo + Send + Sync>,
    account_workers_repository: Arc<dyn AccountWorkersRepo + Send + Sync>,
    plan_limit_service: Arc<dyn PlanLimitService + Send + Sync>,
}

impl WorkerServiceDefault {
    pub fn new(
        worker_executor_clients: Arc<dyn WorkerExecutorClients + Send + Sync>,
        template_service: Arc<dyn TemplateService + Send + Sync>,
        routing_table_service: Arc<dyn RoutingTableService + Send + Sync>,
        project_authorisation_service: Arc<dyn ProjectAuthorisationService + Send + Sync>,
        account_connections_repository: Arc<dyn AccountConnectionsRepo + Send + Sync>,
        account_workers_repository: Arc<dyn AccountWorkersRepo + Send + Sync>,
        plan_limit_service: Arc<dyn PlanLimitService + Send + Sync>,
    ) -> Self {
        Self {
            worker_executor_clients,
            template_service,
            routing_table_service,
            project_authorisation_service,
            account_connections_repository,
            account_workers_repository,
            plan_limit_service,
        }
    }

    async fn check_authorization(
        &self,
        template_id: &TemplateId,
        required_action: &ProjectAction,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        if auth.has_role(&Role::Admin) {
            Ok(())
        } else {
            let project_actions = self
                .project_authorisation_service
                .get_by_template(template_id, auth)
                .await?;
            if project_actions.actions.contains(required_action) {
                Ok(())
            } else {
                Err(WorkerError::Unauthorized(format!(
                    "Account don't have access to {} project action {:?}, for worker",
                    template_id, required_action
                )))
            }
        }
    }

    async fn get_plan_limits(&self, template_id: &TemplateId) -> Result<LimitResult, WorkerError> {
        match self
            .plan_limit_service
            .get_template_limits(template_id)
            .await
        {
            Err(err) => {
                error!(
                    "Get plan worker limit of template {} failed {:?}",
                    template_id, err
                );
                Err(err.into())
            }
            Ok(limit_result) => Ok(limit_result),
        }
    }

    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<ResourceLimits, WorkerError> {
        match self
            .plan_limit_service
            .get_resource_limits(account_id, auth)
            .await
        {
            Err(err) => {
                error!(
                    "Getting current resource limits of account {} failed {:?}",
                    account_id, err
                );
                Err(err.into())
            }
            Ok(resource_limits) => Ok(resource_limits),
        }
    }

    async fn check_plan_limits(
        &self,
        template_id: &TemplateId,
    ) -> Result<CheckLimitResult, WorkerError> {
        match self
            .plan_limit_service
            .check_worker_limit(template_id)
            .await
        {
            Err(err) => {
                error!(
                    "Get plan worker limit of template {} failed {:?}",
                    template_id, err
                );
                Err(err.into())
            }
            Ok(check_limit_result) => {
                if check_limit_result.not_in_limit() {
                    Err(WorkerError::LimitExceeded(format!(
                        "Worker limit exceeded (limit: {})",
                        check_limit_result.limit
                    )))
                } else {
                    Ok(check_limit_result)
                }
            }
        }
    }

    async fn update_account_workers(
        &self,
        account_id: &AccountId,
        value: i32,
    ) -> Result<(), WorkerError> {
        match self
            .account_workers_repository
            .update(account_id, value)
            .await
        {
            Err(_) => Err(WorkerError::Internal(format!(
                "Update worker count of {} failed.",
                account_id
            ))),
            Ok(_) => Ok(()),
        }
    }

    async fn try_get_template_version_for_worker(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<i32, WorkerError> {
        match self.get_metadata(worker_id, auth).await {
            Ok(metadata) => Ok(metadata.template_version),
            Err(WorkerError::WorkerNotFound(_)) => Ok(0),
            Err(WorkerError::Golem(GolemError::WorkerNotFound(_))) => Ok(0),
            Err(other) => Err(other),
        }
    }

    async fn get_worker_executor_client(
        &self,
        worker_id: &WorkerId,
    ) -> Result<Option<WorkerExecutorClient<Channel>>, WorkerError> {
        let routing_table = self.routing_table_service.get_routing_table().await?;
        match routing_table.lookup(worker_id) {
            None => Ok(None),
            Some(pod) => {
                let worker_executor_client = self
                    .worker_executor_clients
                    .lookup(pod)
                    .await
                    .map_err(|err| {
                        WorkerError::Internal(format!(
                            "No client for pod {:?} derived from ShardId {} of {:?}. {}",
                            pod,
                            ShardId::from_worker_id(
                                &worker_id.clone().into(),
                                routing_table.number_of_shards.value,
                            ),
                            worker_id,
                            err
                        ))
                    })?;
                Ok(Some(worker_executor_client))
            }
        }
    }

    async fn retry_on_invalid_shard_id<F, In, Out>(
        &self,
        worker_id: &WorkerId,
        i: &In,
        f: F,
    ) -> Result<Out, WorkerError>
    where
        F: for<'b> Fn(
            &'b mut WorkerExecutorClient<Channel>,
            &'b In,
        )
            -> Pin<Box<dyn Future<Output = Result<Out, GolemError>> + 'b + Send>>,
    {
        loop {
            match self.get_worker_executor_client(worker_id).await {
                Ok(Some(mut worker_executor_client)) => {
                    match f(&mut worker_executor_client, i).await {
                        Ok(result) => return Ok(result),
                        Err(GolemError::InvalidShardId(GolemErrorInvalidShardId {
                            shard_id,
                            shard_ids,
                        })) => {
                            info!("InvalidShardId: {} not in {:?}", shard_id, shard_ids);
                            info!("Invalidating routing table");
                            self.routing_table_service.invalidate_routing_table().await;
                            sleep(Duration::from_secs(1)).await;
                        }
                        Err(GolemError::RuntimeError(GolemErrorRuntimeError { details }))
                            if details.contains("UNAVAILABLE")
                                || details.contains("CHANNEL CLOSED")
                                || details.contains("transport error") =>
                        {
                            info!("Worker executor unavailable");
                            info!("Invalidating routing table");
                            self.routing_table_service.invalidate_routing_table().await;
                            sleep(Duration::from_secs(1)).await;
                        }
                        Err(other) => {
                            debug!("Got {:?}, not retrying", other);
                            return Err(WorkerError::Golem(other));
                        }
                    }
                }
                Ok(None) => {
                    info!("No active shards");
                    info!("Invalidating routing table");
                    self.routing_table_service.invalidate_routing_table().await;
                    sleep(Duration::from_secs(1)).await;
                }
                Err(WorkerError::Internal { 0: details })
                    if details.contains("transport error") =>
                {
                    info!("Shard manager unavailable");
                    info!("Invalidating routing table");
                    self.routing_table_service.invalidate_routing_table().await;
                    sleep(Duration::from_secs(1)).await;
                }
                Err(other) => {
                    debug!("Got {}, not retrying", other);
                    return Err(other);
                }
            }
        }
    }
}

#[async_trait]
impl WorkerService for WorkerServiceDefault {
    async fn create(
        &self,
        worker_id: &WorkerId,
        template_version: i32,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        auth: &AccountAuthorisation,
    ) -> Result<VersionedWorkerId, WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let check_limit_result = self.check_plan_limits(&worker_id.template_id).await?;
        let resource_limits = self
            .get_resource_limits(&check_limit_result.account_id, auth)
            .await?;
        self.update_account_workers(&check_limit_result.account_id, 1)
            .await?;
        self.retry_on_invalid_shard_id(
            &worker_id.clone(),
            &(worker_id.clone(), template_version, arguments, environment_variables, check_limit_result, resource_limits),
            |worker_executor_client, (worker_id, template_version, args, env, check_limit_result, resource_limits)| {
                Box::pin(async move {
                    let response: tonic::Response<workerexecutor::CreateWorkerResponse> = worker_executor_client
                        .create_worker(
                            CreateWorkerRequest {
                                worker_id: Some(worker_id.clone().into()),
                                template_version: *template_version,
                                args: args.clone(),
                                env: env.clone(),
                                account_id: Some(check_limit_result.account_id.clone().into()),
                                account_limits: Some(resource_limits.clone().into()),
                            }
                        )
                        .await
                        .map_err(|err| {
                            GolemError::RuntimeError(GolemErrorRuntimeError {
                                details: err.to_string(),
                            })
                        })?;
                    match response.into_inner() {
                        workerexecutor::CreateWorkerResponse {
                            result:
                            Some(workerexecutor::create_worker_response::Result::Success(_))
                        } => Ok(()),
                        workerexecutor::CreateWorkerResponse {
                            result:
                            Some(workerexecutor::create_worker_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::CreateWorkerResponse { .. } => Err(GolemError::Unknown(GolemErrorUnknown {
                            details: "Empty response".to_string(),
                        }))
                    }
                })
            }).await?;
        Ok(VersionedWorkerId {
            worker_id: worker_id.clone(),
            template_version_used: template_version,
        })
    }

    async fn connect(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<ConnectWorkerStream, WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let plan_limits = self.get_plan_limits(&worker_id.template_id).await?;
        let resource_limits = self
            .get_resource_limits(&plan_limits.account_id, auth)
            .await?;
        match self.get_worker_executor_client(worker_id).await? {
            Some(mut worker_executor_client) => {
                increment_account_connections(
                    self.account_connections_repository.clone(),
                    &auth.token.account_id,
                )
                .await?;
                let response = match worker_executor_client
                    .connect_worker(ConnectWorkerRequest {
                        worker_id: Some(worker_id.clone().into()),
                        account_id: Some(plan_limits.account_id.clone().into()),
                        account_limits: Some(resource_limits.clone().into()),
                    })
                    .await
                {
                    Ok(response) => Ok(response),
                    Err(status) => {
                        decrement_account_connections(
                            self.account_connections_repository.clone(),
                            &auth.token.account_id,
                        )
                        .await?;
                        if status.code() == tonic::Code::NotFound {
                            Err(WorkerError::WorkerNotFound(worker_id.clone()))
                        } else {
                            Err(WorkerError::Internal(status.message().to_string()))
                        }
                    }
                }?;
                Ok(ConnectWorkerStream::new(
                    response.into_inner(),
                    self.account_connections_repository.clone(),
                    auth.token.account_id.clone(),
                ))
            }
            None => Err(WorkerError::WorkerNotFound(worker_id.clone())),
        }
    }

    async fn delete(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::DeleteWorker, auth)
            .await?;
        let plan_limit = self.check_plan_limits(&worker_id.template_id).await?;
        self.retry_on_invalid_shard_id(
            worker_id,
            worker_id,
            |worker_executor_client, worker_id| {
                Box::pin(async move {
                    let response = worker_executor_client
                        .delete_worker(golem_api_grpc::proto::golem::worker::WorkerId::from(
                            worker_id.clone(),
                        ))
                        .await
                        .map_err(|err| {
                            GolemError::RuntimeError(GolemErrorRuntimeError {
                                details: err.to_string(),
                            })
                        })?;
                    match response.into_inner() {
                        workerexecutor::DeleteWorkerResponse {
                            result: Some(workerexecutor::delete_worker_response::Result::Success(_)),
                        } => Ok(()),
                        workerexecutor::DeleteWorkerResponse {
                            result: Some(workerexecutor::delete_worker_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::DeleteWorkerResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            },
        )
        .await?;
        self.update_account_workers(&plan_limit.account_id, -1)
            .await?;
        Ok(())
    }

    async fn get_invocation_key(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<InvocationKey, WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let invocation_key = self
            .retry_on_invalid_shard_id(worker_id, worker_id, |worker_executor_client, worker_id| {
                Box::pin(async move {
                    let response = worker_executor_client
                        .get_invocation_key(GetInvocationKeyRequest {
                            worker_id: Some(worker_id.clone().into()),
                        })
                        .await
                        .map_err(|err| {
                            GolemError::RuntimeError(GolemErrorRuntimeError {
                                details: err.to_string(),
                            })
                        })?;
                    match response.into_inner() {
                        workerexecutor::GetInvocationKeyResponse {
                            result:
                                Some(workerexecutor::get_invocation_key_response::Result::Success(
                                    workerexecutor::GetInvocationKeySuccess {
                                        invocation_key: Some(invocation_key),
                                    },
                                )),
                        } => Ok(invocation_key.into()),
                        workerexecutor::GetInvocationKeyResponse {
                            result:
                                Some(workerexecutor::get_invocation_key_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::GetInvocationKeyResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            })
            .await?;
        Ok(invocation_key)
    }

    async fn invoke_and_await_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Value,
        calling_convention: &CallingConvention,
        auth: &AccountAuthorisation,
    ) -> Result<Value, WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let template_version = self
            .try_get_template_version_for_worker(worker_id, auth)
            .await?;
        let template_details = self
            .template_service
            .get_by_version(
                &VersionedTemplateId {
                    template_id: worker_id.template_id.clone(),
                    version: template_version,
                },
                auth,
            )
            .await?
            .ok_or_else(|| {
                WorkerError::VersionedTemplateIdNotFound(VersionedTemplateId {
                    template_id: worker_id.template_id.clone(),
                    version: template_version,
                })
            })?;
        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerError::TypeCheckerError("Failed to find the function".to_string())
            })?;
        let params_val = params
            .validate_function_parameters(
                function_type
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.into())
                    .collect(),
                calling_convention.clone(),
            )
            .map_err(|err| WorkerError::TypeCheckerError(err.join(", ")))?;
        let results_val = self
            .invoke_and_await_function_proto(
                worker_id,
                function_name,
                invocation_key,
                params_val,
                calling_convention,
                auth,
            )
            .await?;

        let function_results: Vec<FunctionResult> = function_type
            .results
            .iter()
            .map(|x| x.clone().into())
            .collect();

        let invoke_response_json = results_val
            .result
            .validate_function_result(function_results, calling_convention.clone())
            .map_err(|err| WorkerError::TypeCheckerError(err.join(", ")))?;
        Ok(invoke_response_json)
    }

    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        invocation_key: &InvocationKey,
        params: Vec<ProtoVal>,
        calling_convention: &CallingConvention,
        auth: &AccountAuthorisation,
    ) -> Result<ProtoInvokeResult, WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let template_version = self
            .try_get_template_version_for_worker(worker_id, auth)
            .await?;
        let template_details = self
            .template_service
            .get_by_version(
                &VersionedTemplateId {
                    template_id: worker_id.template_id.clone(),
                    version: template_version,
                },
                auth,
            )
            .await?
            .ok_or_else(|| {
                WorkerError::VersionedTemplateIdNotFound(VersionedTemplateId {
                    template_id: worker_id.template_id.clone(),
                    version: template_version,
                })
            })?;
        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerError::TypeCheckerError("Failed to find the function".to_string())
            })?;
        let params_val = params
            .validate_function_parameters(
                function_type
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.into())
                    .collect(),
                calling_convention.clone(),
            )
            .map_err(|err| WorkerError::TypeCheckerError(err.join(", ")))?;
        let plan_limits = self.get_plan_limits(&worker_id.template_id).await?;
        let resource_limits = self
            .get_resource_limits(&plan_limits.account_id, auth)
            .await?;
        let invoke_response = self.retry_on_invalid_shard_id(
            worker_id,
            &(worker_id.clone(), function_name, params_val, invocation_key.clone(), calling_convention.clone(), plan_limits.account_id, resource_limits),
            |worker_executor_client, (worker_id, function_name, params_val, invocation_key, calling_convention, account_id, resource_limits)| {
                Box::pin(async move {
                    let response = worker_executor_client.invoke_and_await_worker(
                        InvokeAndAwaitWorkerRequest {
                            worker_id: Some(worker_id.clone().into()),
                            name: function_name.clone(),
                            input: params_val.clone(),
                            invocation_key: Some(invocation_key.clone().into()),
                            calling_convention: calling_convention.clone().into(),
                            account_id: Some(account_id.clone().into()),
                            account_limits: Some(resource_limits.clone().into()),
                        }
                    ).await.map_err(|err| {
                        GolemError::RuntimeError(GolemErrorRuntimeError {
                            details: err.to_string(),
                        })
                    })?;
                    match response.into_inner() {
                        workerexecutor::InvokeAndAwaitWorkerResponse {
                            result:
                            Some(workerexecutor::invoke_and_await_worker_response::Result::Success(
                                     workerexecutor::InvokeAndAwaitWorkerSuccess {
                                         output,
                                     },
                                 )),
                        } => Ok(ProtoInvokeResult { result: output }),
                        workerexecutor::InvokeAndAwaitWorkerResponse {
                            result:
                            Some(workerexecutor::invoke_and_await_worker_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::InvokeAndAwaitWorkerResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            },
        ).await?;
        debug!("Invoke_response: {:?}", invoke_response);
        Ok(invoke_response)
    }

    async fn invoke_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Value,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let template_version = self
            .try_get_template_version_for_worker(worker_id, auth)
            .await?;
        let template_details = self
            .template_service
            .get_by_version(
                &VersionedTemplateId {
                    template_id: worker_id.template_id.clone(),
                    version: template_version,
                },
                auth,
            )
            .await?
            .ok_or_else(|| {
                WorkerError::VersionedTemplateIdNotFound(VersionedTemplateId {
                    template_id: worker_id.template_id.clone(),
                    version: template_version,
                })
            })?;
        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerError::TypeCheckerError("Failed to find the function".to_string())
            })?;
        let params_val = params
            .validate_function_parameters(
                function_type
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.into())
                    .collect(),
                CallingConvention::Component,
            )
            .map_err(|err| WorkerError::TypeCheckerError(err.join(", ")))?;
        self.invoke_fn_proto(worker_id, function_name.clone(), params_val, auth)
            .await?;
        Ok(())
    }

    async fn invoke_fn_proto(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Vec<ProtoVal>,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let template_version = self
            .try_get_template_version_for_worker(worker_id, auth)
            .await?;
        let template_details = self
            .template_service
            .get_by_version(
                &VersionedTemplateId {
                    template_id: worker_id.template_id.clone(),
                    version: template_version,
                },
                auth,
            )
            .await?
            .ok_or_else(|| {
                WorkerError::VersionedTemplateIdNotFound(VersionedTemplateId {
                    template_id: worker_id.template_id.clone(),
                    version: template_version,
                })
            })?;
        let function_type = template_details
            .metadata
            .function_by_name(&function_name)
            .ok_or_else(|| {
                WorkerError::TypeCheckerError("Failed to find the function".to_string())
            })?;
        let params_val = params
            .validate_function_parameters(
                function_type
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.into())
                    .collect(),
                CallingConvention::Component,
            )
            .map_err(|err| WorkerError::TypeCheckerError(err.join(", ")))?;
        let plan_limits = self.get_plan_limits(&worker_id.template_id).await?;
        let resource_limits = self
            .get_resource_limits(&plan_limits.account_id, auth)
            .await?;
        self.retry_on_invalid_shard_id(
            worker_id,
            &(
                worker_id.clone(),
                function_name,
                params_val,
                plan_limits.account_id,
                resource_limits,
            ),
            |worker_executor_client,
             (worker_id, function_name, params_val, account_id, resource_limits)| {
                Box::pin(async move {
                    let response = worker_executor_client
                        .invoke_worker(InvokeWorkerRequest {
                            worker_id: Some(worker_id.clone().into()),
                            name: function_name.clone(),
                            input: params_val.clone(),
                            account_id: Some(account_id.clone().into()),
                            account_limits: Some(resource_limits.clone().into()),
                        })
                        .await
                        .map_err(|err| {
                            GolemError::RuntimeError(GolemErrorRuntimeError {
                                details: err.to_string(),
                            })
                        })?;
                    match response.into_inner() {
                        workerexecutor::InvokeWorkerResponse {
                            result: Some(workerexecutor::invoke_worker_response::Result::Success(_)),
                        } => Ok(()),
                        workerexecutor::InvokeWorkerResponse {
                            result: Some(workerexecutor::invoke_worker_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::InvokeWorkerResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            },
        )
        .await?;
        Ok(())
    }

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: i32,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<bool, WorkerError> {
        let promise_id = PromiseId {
            worker_id: worker_id.clone(),
            oplog_idx: oplog_id,
        };
        self.check_authorization(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let result = self
            .retry_on_invalid_shard_id(
                worker_id,
                &(promise_id, data),
                |worker_executor_client, (promise_id, data)| {
                    Box::pin(async move {
                        let response = worker_executor_client
                            .complete_promise(CompletePromiseRequest {
                                promise_id: Some(promise_id.clone().into()),
                                data: data.clone(),
                            })
                            .await
                            .map_err(|err| {
                                GolemError::RuntimeError(GolemErrorRuntimeError {
                                    details: err.to_string(),
                                })
                            })?;
                        match response.into_inner() {
                            workerexecutor::CompletePromiseResponse {
                                result:
                                    Some(workerexecutor::complete_promise_response::Result::Success(
                                        success,
                                    )),
                            } => Ok(success.completed),
                            workerexecutor::CompletePromiseResponse {
                                result:
                                    Some(workerexecutor::complete_promise_response::Result::Failure(
                                        err,
                                    )),
                            } => Err(err.try_into().unwrap()),
                            workerexecutor::CompletePromiseResponse { .. } => {
                                Err(GolemError::Unknown(GolemErrorUnknown {
                                    details: "Empty response".to_string(),
                                }))
                            }
                        }
                    })
                },
            )
            .await?;
        Ok(result)
    }

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::UpdateWorker, auth)
            .await?;
        self.retry_on_invalid_shard_id(
            worker_id,
            worker_id,
            |worker_executor_client, worker_id| {
                Box::pin(async move {
                    let response = worker_executor_client
                        .interrupt_worker(InterruptWorkerRequest {
                            worker_id: Some(worker_id.clone().into()),
                            recover_immediately,
                        })
                        .await
                        .map_err(|err| {
                            GolemError::RuntimeError(GolemErrorRuntimeError {
                                details: err.to_string(),
                            })
                        })?;
                    match response.into_inner() {
                        workerexecutor::InterruptWorkerResponse {
                            result: Some(workerexecutor::interrupt_worker_response::Result::Success(_)),
                        } => Ok(()),
                        workerexecutor::InterruptWorkerResponse {
                            result: Some(workerexecutor::interrupt_worker_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::InterruptWorkerResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            },
        )
        .await?;
        Ok(())
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::WorkerMetadata, WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::ViewWorker, auth)
            .await?;
        let metadata = self.retry_on_invalid_shard_id(
            worker_id,
            worker_id,
            |worker_executor_client, worker_id| {
                Box::pin(async move {
                    let response = worker_executor_client.get_worker_metadata(
                        golem_api_grpc::proto::golem::worker::WorkerId::from(worker_id.clone())
                    ).await.map_err(|err| {
                        GolemError::RuntimeError(GolemErrorRuntimeError {
                            details: err.to_string(),
                        })
                    })?;
                    match response.into_inner() {
                        workerexecutor::GetWorkerMetadataResponse {
                            result:
                            Some(workerexecutor::get_worker_metadata_response::Result::Success(metadata)),
                        } => Ok(metadata.try_into().unwrap()),
                        workerexecutor::GetWorkerMetadataResponse {
                            result:
                            Some(workerexecutor::get_worker_metadata_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::GetWorkerMetadataResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            },
        ).await?;
        Ok(metadata)
    }

    async fn resume(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        self.check_authorization(&worker_id.template_id, &ProjectAction::UpdateWorker, auth)
            .await?;
        self.retry_on_invalid_shard_id(
            worker_id,
            worker_id,
            |worker_executor_client, worker_id| {
                Box::pin(async move {
                    let response = worker_executor_client
                        .resume_worker(ResumeWorkerRequest {
                            worker_id: Some(worker_id.clone().into()),
                        })
                        .await
                        .map_err(|err| {
                            GolemError::RuntimeError(GolemErrorRuntimeError {
                                details: err.to_string(),
                            })
                        })?;
                    match response.into_inner() {
                        workerexecutor::ResumeWorkerResponse {
                            result: Some(workerexecutor::resume_worker_response::Result::Success(_)),
                        } => Ok(()),
                        workerexecutor::ResumeWorkerResponse {
                            result: Some(workerexecutor::resume_worker_response::Result::Failure(err)),
                        } => Err(err.try_into().unwrap()),
                        workerexecutor::ResumeWorkerResponse { .. } => {
                            Err(GolemError::Unknown(GolemErrorUnknown {
                                details: "Empty response".to_string(),
                            }))
                        }
                    }
                })
            },
        )
        .await?;
        Ok(())
    }
}

async fn increment_account_connections(
    account_connections_repository: Arc<dyn AccountConnectionsRepo + Send + Sync>,
    account_id: &AccountId,
) -> Result<(), WorkerError> {
    match account_connections_repository.update(account_id, 1).await {
        Ok(connections) => {
            if connections > 10 {
                decrement_account_connections(account_connections_repository, account_id).await?;
                Err(WorkerError::LimitExceeded(
                    "Worker limit exceeded (limit: 10)".to_string(),
                ))
            } else {
                Ok(())
            }
        }
        Err(err) => {
            error!(
                "Increment active connections of account {} failed {}",
                account_id, err
            );
            Err(WorkerError::Internal("Unexpected error".to_string()))
        }
    }
}

async fn decrement_account_connections(
    account_connections_repository: Arc<dyn AccountConnectionsRepo + Send + Sync>,
    account_id: &AccountId,
) -> Result<(), WorkerError> {
    match account_connections_repository.update(account_id, -1).await {
        Ok(_) => Ok(()),
        Err(err) => {
            error!(
                "Decrement active connections of account {} failed {}",
                account_id, err
            );
            Err(WorkerError::Internal("Unexpected error".to_string()))
        }
    }
}

#[derive(Default)]
pub struct WorkerServiceNoOp {}

#[async_trait]
impl WorkerService for WorkerServiceNoOp {
    async fn create(
        &self,
        worker_id: &WorkerId,
        _template_version: i32,
        _arguments: Vec<String>,
        _environment_variables: HashMap<String, String>,
        _auth: &AccountAuthorisation,
    ) -> Result<VersionedWorkerId, WorkerError> {
        Ok(VersionedWorkerId {
            worker_id: worker_id.clone(),
            template_version_used: 0,
        })
    }

    async fn connect(
        &self,
        _worker_id: &WorkerId,
        _auth: &AccountAuthorisation,
    ) -> Result<ConnectWorkerStream, WorkerError> {
        Err(WorkerError::Internal("Not supported".to_string()))
    }

    async fn delete(
        &self,
        _worker_id: &WorkerId,
        _auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn get_invocation_key(
        &self,
        _worker_id: &WorkerId,
        _auth: &AccountAuthorisation,
    ) -> Result<InvocationKey, WorkerError> {
        Ok(InvocationKey {
            value: "".to_string(),
        })
    }

    async fn invoke_and_await_function(
        &self,
        _worker_id: &WorkerId,
        _function_name: String,
        _invocation_key: &InvocationKey,
        _params: Value,
        _calling_convention: &CallingConvention,
        _auth: &AccountAuthorisation,
    ) -> Result<Value, WorkerError> {
        Ok(Value::Null)
    }

    async fn invoke_and_await_function_proto(
        &self,
        _worker_id: &WorkerId,
        _function_name: String,
        _invocation_key: &InvocationKey,
        _params: Vec<ProtoVal>,
        _calling_convention: &CallingConvention,
        _auth: &AccountAuthorisation,
    ) -> Result<ProtoInvokeResult, WorkerError> {
        Ok(ProtoInvokeResult { result: vec![] })
    }

    async fn invoke_function(
        &self,
        _worker_id: &WorkerId,
        _function_name: String,
        _params: Value,
        _auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn invoke_fn_proto(
        &self,
        _worker_id: &WorkerId,
        _function_name: String,
        _params: Vec<ProtoVal>,
        _auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn complete_promise(
        &self,
        _worker_id: &WorkerId,
        _oplog_id: i32,
        _data: Vec<u8>,
        _auth: &AccountAuthorisation,
    ) -> Result<bool, WorkerError> {
        Ok(true)
    }

    async fn interrupt(
        &self,
        _worker_id: &WorkerId,
        _recover_immediately: bool,
        _auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::WorkerMetadata, WorkerError> {
        Ok(crate::model::WorkerMetadata {
            worker_id: worker_id.clone(),
            account_id: auth.token.account_id.clone(),
            args: vec![],
            env: Default::default(),
            status: WorkerStatus::Running,
            template_version: 0,
            retry_count: 0,
        })
    }

    async fn resume(
        &self,
        _worker_id: &WorkerId,
        _auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}
