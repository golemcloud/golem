use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::InvokeResult as ProtoInvokeResult;
use golem_common::model::{
    AccountId, CallingConvention, InvocationKey, ProjectId, TemplateId, Timestamp, WorkerFilter,
    WorkerStatus,
};
use golem_wasm_rpc::protobuf::Val as ProtoVal;
use golem_worker_service_base::service::worker::{
    WorkerRequestMetadata, WorkerService as BaseWorkerService,
    WorkerServiceError as BaseWorkerServiceError,
};
use serde_json::Value;

use crate::auth::AccountAuthorisation;
use crate::model::*;
use crate::repo::account_connections::AccountConnectionsRepo;
use crate::repo::account_workers::AccountWorkersRepo;
use golem_service_base::model::*;

mod connect;
mod template;

pub use connect::*;
pub use template::*;

use super::{
    plan_limit::{CheckLimitResult, LimitResult, PlanLimitError, PlanLimitService},
    project_auth::{ProjectAuthorisationError, ProjectAuthorisationService},
};

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("Project Not Found: {0}")]
    ProjectNotFound(ProjectId),
    #[error(transparent)]
    Base(#[from] BaseWorkerServiceError),
}

#[async_trait]
pub trait WorkerService {
    async fn create(
        &self,
        worker_id: &WorkerId,
        template_version: u64,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        auth: &AccountAuthorisation,
    ) -> Result<WorkerId, WorkerError>;

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
        oplog_id: u64,
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

    async fn find_metadata(
        &self,
        template_id: &TemplateId,
        filter: Option<WorkerFilter>,
        cursor: u64,
        count: u64,
        precise: bool,
        auth: &AccountAuthorisation,
    ) -> Result<(Option<u64>, Vec<crate::model::WorkerMetadata>), WorkerError>;

    async fn resume(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError>;
}

#[derive(Clone)]
pub struct WorkerServiceDefault {
    base_worker_service: Arc<dyn BaseWorkerService<AccountAuthorisation> + Send + Sync>,

    project_authorisation_service: Arc<dyn ProjectAuthorisationService + Send + Sync>,
    plan_limit_service: Arc<dyn PlanLimitService + Send + Sync>,

    account_connections_repository: Arc<dyn AccountConnectionsRepo + Send + Sync>,
    account_workers_repository: Arc<dyn AccountWorkersRepo + Send + Sync>,
}

impl WorkerServiceDefault {
    pub fn new(
        base_worker_service: Arc<dyn BaseWorkerService<AccountAuthorisation> + Send + Sync>,
        project_authorisation_service: Arc<dyn ProjectAuthorisationService + Send + Sync>,
        plan_limit_service: Arc<dyn PlanLimitService + Send + Sync>,
        account_connections_repository: Arc<dyn AccountConnectionsRepo + Send + Sync>,
        account_workers_repository: Arc<dyn AccountWorkersRepo + Send + Sync>,
    ) -> Self {
        WorkerServiceDefault {
            base_worker_service,
            project_authorisation_service,
            plan_limit_service,
            account_connections_repository,
            account_workers_repository,
        }
    }

    async fn update_account_workers(&self, account_id: &AccountId, value: i32) {
        let result = self
            .account_workers_repository
            .update(account_id, value)
            .await;
        if result.is_err() {
            tracing::error!("Update worker count of {account_id} failed.");
        }
    }

    async fn increment_account_connections(
        &self,
        account_id: &AccountId,
    ) -> Result<(), WorkerError> {
        match self
            .account_connections_repository
            .update(account_id, 1)
            .await
        {
            Ok(connections) => {
                if connections > 10 {
                    self.decrement_account_connections(account_id).await;
                    let err =
                        WorkerError::Forbidden("Worker limit exceeded (limit: 10)".to_string());
                    Err(err)
                } else {
                    Ok(())
                }
            }
            Err(err) => {
                tracing::error!(
                    "Increment active connections of account {account_id} failed {err:?}",
                );
                // TODO: Should this error propagate?
                Ok(())
            }
        }
    }

    async fn decrement_account_connections(&self, account_id: &AccountId) {
        let result = self
            .account_connections_repository
            .update(account_id, -1)
            .await;
        if result.is_err() {
            tracing::error!("Decrement worker count of {account_id} failed.");
        }
    }
}

#[async_trait]
impl WorkerService for WorkerServiceDefault {
    async fn create(
        &self,
        worker_id: &WorkerId,
        template_version: u64,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        auth: &AccountAuthorisation,
    ) -> Result<WorkerId, WorkerError> {
        let namespace = self
            .authorize(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .create(
                worker_id,
                template_version,
                arguments,
                environment_variables,
                namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        self.update_account_workers(&namespace.worker_limit_result.account_id, 1)
            .await;

        Ok(value)
    }

    async fn connect(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<ConnectWorkerStream, WorkerError> {
        let namespace = self
            .authorize(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .connect(worker_id, namespace.as_worker_request_metadata(), auth)
            .await?;

        self.increment_account_connections(&namespace.worker_limit_result.account_id)
            .await?;

        Ok(ConnectWorkerStream::new(
            value,
            self.account_connections_repository.clone(),
            namespace.account_id,
        ))
    }

    async fn delete(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        let namespace = self
            .authorize(&worker_id.template_id, &ProjectAction::DeleteWorker, auth)
            .await?;

        self.base_worker_service.delete(worker_id, auth).await?;

        self.update_account_workers(&namespace.worker_limit_result.account_id, -1)
            .await;

        Ok(())
    }

    async fn get_invocation_key(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<InvocationKey, WorkerError> {
        let _ = self
            .authorize(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .get_invocation_key(worker_id, auth)
            .await?;

        Ok(value)
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
        let namespace = self
            .authorize(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .invoke_and_await_function(
                worker_id,
                function_name,
                invocation_key,
                params,
                calling_convention,
                namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        Ok(value)
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
        let namespace = self
            .authorize(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .invoke_and_await_function_proto(
                worker_id,
                function_name,
                invocation_key,
                params,
                calling_convention,
                namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        Ok(value)
    }

    async fn invoke_function(
        &self,
        worker_id: &WorkerId,
        function_name: String,
        params: Value,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        let namespace = self
            .authorize(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let _ = self
            .base_worker_service
            .invoke_function(
                worker_id,
                function_name,
                params,
                namespace.as_worker_request_metadata(),
                auth,
            )
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
        let namespace = self
            .authorize(&worker_id.template_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let _ = self
            .base_worker_service
            .invoke_fn_proto(
                worker_id,
                function_name,
                params,
                namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        Ok(())
    }

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<bool, WorkerError> {
        let _ = self
            .authorize(&worker_id.template_id, &ProjectAction::UpdateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .complete_promise(worker_id, oplog_id, data, auth)
            .await?;

        Ok(value)
    }

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        let _ = self
            .authorize(&worker_id.template_id, &ProjectAction::UpdateWorker, auth)
            .await?;

        self.base_worker_service
            .interrupt(worker_id, recover_immediately, auth)
            .await?;

        Ok(())
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::WorkerMetadata, WorkerError> {
        let namespace = self
            .authorize(&worker_id.template_id, &ProjectAction::ViewWorker, auth)
            .await?;

        let metadata = self
            .base_worker_service
            .get_metadata(worker_id, auth)
            .await?;

        let metadata = convert_metadata(metadata, namespace.account_id);

        Ok(metadata)
    }

    async fn find_metadata(
        &self,
        template_id: &TemplateId,
        filter: Option<WorkerFilter>,
        cursor: u64,
        count: u64,
        precise: bool,
        auth: &AccountAuthorisation,
    ) -> Result<(Option<u64>, Vec<crate::model::WorkerMetadata>), WorkerError> {
        let namespace = self
            .authorize(template_id, &ProjectAction::ViewWorker, auth)
            .await?;

        let (pagination, metadata) = self
            .base_worker_service
            .find_metadata(template_id, filter, cursor, count, precise, auth)
            .await?;

        let metadata = metadata
            .into_iter()
            .map(|metadata| convert_metadata(metadata, namespace.account_id.clone()))
            .collect();

        Ok((pagination, metadata))
    }

    async fn resume(
        &self,
        worker_id: &WorkerId,
        auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        let _ = self.base_worker_service.resume(worker_id, auth).await?;

        Ok(())
    }
}

fn convert_metadata(
    metadata: golem_service_base::model::WorkerMetadata,
    account_id: AccountId,
) -> crate::model::WorkerMetadata {
    crate::model::WorkerMetadata {
        account_id,
        worker_id: metadata.worker_id,
        args: metadata.args,
        env: metadata.env,
        status: metadata.status,
        template_version: metadata.template_version,
        retry_count: metadata.retry_count,
        pending_invocation_count: metadata.pending_invocation_count,
        updates: metadata.updates,
        created_at: metadata.created_at,
        last_error: metadata.last_error,
    }
}

#[derive(Clone)]
pub struct WorkerNamespace {
    pub account_id: AccountId,
    pub template_limits: LimitResult,
    pub resource_limits: golem_service_base::model::ResourceLimits,
    pub worker_limit_result: CheckLimitResult,
}

impl WorkerNamespace {
    fn as_worker_request_metadata(&self) -> WorkerRequestMetadata {
        WorkerRequestMetadata {
            account_id: Some(self.account_id.clone()),
            limits: Some(self.resource_limits.clone()),
        }
    }
}

impl WorkerServiceDefault {
    async fn authorize(
        &self,
        template: &TemplateId,
        action: &ProjectAction,
        auth: &AccountAuthorisation,
    ) -> Result<WorkerNamespace, WorkerError> {
        let (_, template_limits, worker_limit_result) = tokio::try_join!(
            self.check_authorization(template, action, auth),
            self.get_template_limits(template),
            self.check_worker_limit(template)
        )?;

        let resource_limits = self
            .get_resource_limits(&template_limits.account_id, auth)
            .await?;

        Ok(WorkerNamespace {
            account_id: worker_limit_result.account_id.clone(),
            template_limits,
            resource_limits,
            worker_limit_result,
        })
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
                Err(WorkerError::Forbidden(format!(
                    "Account don't have access to {} project action {:?}, for worker",
                    template_id, required_action
                )))
            }
        }
    }

    async fn get_template_limits(
        &self,
        template_id: &TemplateId,
    ) -> Result<LimitResult, WorkerError> {
        match self
            .plan_limit_service
            .get_template_limits(template_id)
            .await
        {
            Err(err) => {
                tracing::error!("Get plan worker limit of template {template_id} failed {err:?}",);
                Err(err.into())
            }
            Ok(limit_result) => Ok(limit_result),
        }
    }

    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<golem_service_base::model::ResourceLimits, WorkerError> {
        match self
            .plan_limit_service
            .get_resource_limits(account_id, auth)
            .await
        {
            Err(err) => {
                tracing::error!(
                    "Getting current resource limits of account {account_id} failed {err:?}",
                );
                Err(err.into())
            }
            Ok(resource_limits) => {
                let limits = golem_service_base::model::ResourceLimits {
                    available_fuel: resource_limits.available_fuel,
                    max_memory_per_worker: resource_limits.max_memory_per_worker,
                };

                Ok(limits)
            }
        }
    }

    async fn check_worker_limit(
        &self,
        template_id: &TemplateId,
    ) -> Result<CheckLimitResult, WorkerError> {
        match self
            .plan_limit_service
            .check_worker_limit(template_id)
            .await
        {
            Err(err) => {
                tracing::error!("Get plan worker limit of template {template_id} failed {err:?}",);
                Err(err.into())
            }
            Ok(check_limit_result) => {
                if check_limit_result.not_in_limit() {
                    Err(WorkerError::Forbidden(format!(
                        "Worker limit exceeded (limit: {})",
                        check_limit_result.limit
                    )))
                } else {
                    Ok(check_limit_result)
                }
            }
        }
    }
}

impl From<ProjectAuthorisationError> for WorkerError {
    fn from(error: ProjectAuthorisationError) -> Self {
        match error {
            ProjectAuthorisationError::Internal(error) => {
                WorkerError::Base(BaseWorkerServiceError::Internal(anyhow::Error::msg(error)))
            }
            ProjectAuthorisationError::Unauthorized(error) => WorkerError::Unauthorized(error),
        }
    }
}

impl From<PlanLimitError> for WorkerError {
    fn from(error: PlanLimitError) -> Self {
        match error {
            PlanLimitError::AccountIdNotFound(account_id) => {
                WorkerError::Base(BaseWorkerServiceError::AccountIdNotFound(account_id))
            }
            PlanLimitError::ProjectIdNotFound(project_id) => {
                WorkerError::ProjectNotFound(project_id)
            }
            PlanLimitError::TemplateIdNotFound(template_id) => {
                WorkerError::Base(BaseWorkerServiceError::TemplateNotFound(template_id))
            }
            PlanLimitError::Unauthorized(string) => WorkerError::Unauthorized(string),
            PlanLimitError::Internal(e) => {
                WorkerError::Base(BaseWorkerServiceError::Internal(anyhow::Error::msg(e)))
            }
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
        _template_version: u64,
        _arguments: Vec<String>,
        _environment_variables: HashMap<String, String>,
        _auth: &AccountAuthorisation,
    ) -> Result<WorkerId, WorkerError> {
        Ok(worker_id.clone())
    }

    async fn connect(
        &self,
        _worker_id: &WorkerId,
        _auth: &AccountAuthorisation,
    ) -> Result<ConnectWorkerStream, WorkerError> {
        Err(WorkerError::Base(BaseWorkerServiceError::Internal(
            anyhow::Error::msg("Not supported"),
        )))
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
        _oplog_id: u64,
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
            created_at: Timestamp::now_utc(),
            pending_invocation_count: 0,
            updates: vec![],
            last_error: None,
        })
    }

    async fn find_metadata(
        &self,
        _template_id: &TemplateId,
        _filter: Option<WorkerFilter>,
        _cursor: u64,
        _count: u64,
        _precise: bool,
        _auth: &AccountAuthorisation,
    ) -> Result<(Option<u64>, Vec<crate::model::WorkerMetadata>), WorkerError> {
        Ok((None, vec![]))
    }

    async fn resume(
        &self,
        _worker_id: &WorkerId,
        _auth: &AccountAuthorisation,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}
