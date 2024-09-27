use crate::service::auth::AuthService;
use async_trait::async_trait;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::clients::auth::AuthServiceError;
use cloud_common::clients::limit::{LimitError, LimitService};
use cloud_common::model::ProjectAction;
use cloud_common::SafeDisplay;
use golem_api_grpc::proto::golem::worker::{
    IdempotencyKey as ProtoIdempotencyKey, InvocationContext, InvokeResult as ProtoInvokeResult,
    UpdateMode,
};
use golem_common::model::{
    AccountId, ComponentId, ComponentVersion, IdempotencyKey, ProjectId, ScanCursor,
    TargetWorkerId, WorkerFilter, WorkerId,
};
use golem_service_base::model::*;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::Val as ProtoVal;
use golem_worker_service_base::service::worker::{
    WorkerRequestMetadata, WorkerService as BaseWorkerService,
    WorkerServiceError as BaseWorkerServiceError,
};
use std::collections::HashMap;
use std::sync::Arc;

mod connect;
pub use connect::*;

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
    #[error(transparent)]
    InternalAuthServiceError(AuthServiceError),
}

impl SafeDisplay for WorkerError {
    fn to_safe_string(&self) -> String {
        match self {
            WorkerError::Unauthorized(_) => self.to_string(),
            WorkerError::Forbidden(_) => self.to_string(),
            WorkerError::ProjectNotFound(_) => self.to_string(),
            WorkerError::Base(error) => error.to_string(), // TODO: Implement SafeDisplay for BaseWorkerServiceError
            WorkerError::InternalAuthServiceError(error) => error.to_safe_string(),
        }
    }
}

impl From<AuthServiceError> for WorkerError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::Unauthorized(error) => WorkerError::Unauthorized(error),
            AuthServiceError::Forbidden(error) => WorkerError::Forbidden(error),
            AuthServiceError::InternalClientError(_) => {
                WorkerError::InternalAuthServiceError(value)
            }
        }
    }
}

impl From<LimitError> for WorkerError {
    fn from(error: LimitError) -> Self {
        match error {
            LimitError::Unauthorized(message) => WorkerError::Unauthorized(message),
            LimitError::LimitExceeded(message) => WorkerError::Forbidden(message),
            LimitError::InternalClientError(message) => WorkerError::Base(
                BaseWorkerServiceError::Internal(anyhow::Error::msg(message)),
            ),
        }
    }
}
#[async_trait]
pub trait WorkerService {
    async fn create(
        &self,
        worker_id: &WorkerId,
        component_version: u64,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        auth: &CloudAuthCtx,
    ) -> Result<WorkerId, WorkerError>;

    async fn connect(
        &self,
        worker_id: &WorkerId,
        auth: &CloudAuthCtx,
    ) -> Result<ConnectWorkerStream, WorkerError>;

    async fn delete(&self, worker_id: &WorkerId, auth: &CloudAuthCtx) -> Result<(), WorkerError>;

    async fn invoke_and_await_function_json(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<TypeAnnotatedValue>,
        invocation_context: Option<InvocationContext>,
        auth: &CloudAuthCtx,
    ) -> Result<TypeAnnotatedValue, WorkerError>;

    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        auth: &CloudAuthCtx,
    ) -> Result<ProtoInvokeResult, WorkerError>;

    async fn invoke_function_json(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<TypeAnnotatedValue>,
        invocation_context: Option<InvocationContext>,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError>;

    async fn invoke_function_proto(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError>;

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        auth: &CloudAuthCtx,
    ) -> Result<bool, WorkerError>;

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError>;

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth: &CloudAuthCtx,
    ) -> Result<crate::model::WorkerMetadata, WorkerError>;

    async fn find_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        auth: &CloudAuthCtx,
    ) -> Result<(Option<ScanCursor>, Vec<crate::model::WorkerMetadata>), WorkerError>;

    async fn resume(&self, worker_id: &WorkerId, auth: &CloudAuthCtx) -> Result<(), WorkerError>;

    async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: UpdateMode,
        target_version: ComponentVersion,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError>;

    async fn get_component_for_worker(
        &self,
        worker_id: &WorkerId,
        auth_ctx: &CloudAuthCtx,
    ) -> Result<Component, WorkerError>;
}

#[derive(Clone)]
pub struct WorkerServiceDefault {
    auth_service: Arc<dyn AuthService + Sync + Send>,
    limit_service: Arc<dyn LimitService + Sync + Send>,
    base_worker_service: Arc<dyn BaseWorkerService<CloudAuthCtx> + Send + Sync>,
}

impl WorkerServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService + Sync + Send>,
        limit_service: Arc<dyn LimitService + Sync + Send>,
        base_worker_service: Arc<dyn BaseWorkerService<CloudAuthCtx> + Send + Sync>,
    ) -> Self {
        WorkerServiceDefault {
            auth_service,
            limit_service,
            base_worker_service,
        }
    }

    async fn authorize(
        &self,
        component_id: &ComponentId,
        action: &ProjectAction,
        auth: &CloudAuthCtx,
    ) -> Result<WorkerNamespace, WorkerError> {
        let namespace = self
            .auth_service
            .is_authorized_by_component(component_id, action.clone(), auth)
            .await?;

        let resource_limits = self
            .limit_service
            .get_resource_limits(&namespace.account_id)
            .await?;

        Ok(WorkerNamespace {
            namespace,
            resource_limits: resource_limits.into(),
        })
    }
}

#[async_trait]
impl WorkerService for WorkerServiceDefault {
    async fn create(
        &self,
        worker_id: &WorkerId,
        component_version: u64,
        arguments: Vec<String>,
        environment_variables: HashMap<String, String>,
        auth: &CloudAuthCtx,
    ) -> Result<WorkerId, WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .create(
                worker_id,
                component_version,
                arguments,
                environment_variables,
                worker_namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        self.limit_service
            .update_worker_limit(&worker_namespace.namespace.account_id, worker_id, 1)
            .await?;

        Ok(value)
    }

    async fn connect(
        &self,
        worker_id: &WorkerId,
        auth: &CloudAuthCtx,
    ) -> Result<ConnectWorkerStream, WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .connect(
                worker_id,
                worker_namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        self.limit_service
            .update_worker_connection_limit(&worker_namespace.namespace.account_id, worker_id, 1)
            .await?;

        Ok(ConnectWorkerStream::new(
            value,
            worker_id.clone(),
            worker_namespace.namespace,
            self.limit_service.clone(),
        ))
    }

    async fn delete(&self, worker_id: &WorkerId, auth: &CloudAuthCtx) -> Result<(), WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::DeleteWorker, auth)
            .await?;

        self.base_worker_service
            .delete(
                worker_id,
                worker_namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        self.limit_service
            .update_worker_limit(&worker_namespace.namespace.account_id, worker_id, -1)
            .await?;

        Ok(())
    }

    async fn invoke_and_await_function_json(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<TypeAnnotatedValue>,
        invocation_context: Option<InvocationContext>,
        auth: &CloudAuthCtx,
    ) -> Result<TypeAnnotatedValue, WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .invoke_and_await_function_json(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                worker_namespace.as_worker_request_metadata(),
            )
            .await?;

        Ok(value)
    }

    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        auth: &CloudAuthCtx,
    ) -> Result<ProtoInvokeResult, WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .invoke_and_await_function_proto(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                worker_namespace.as_worker_request_metadata(),
            )
            .await?;

        Ok(value)
    }

    async fn invoke_function_json(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<TypeAnnotatedValue>,
        invocation_context: Option<InvocationContext>,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let _ = self
            .base_worker_service
            .invoke_function_json(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                worker_namespace.as_worker_request_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn invoke_function_proto(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        let namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let _ = self
            .base_worker_service
            .invoke_function_proto(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                namespace.as_worker_request_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        auth: &CloudAuthCtx,
    ) -> Result<bool, WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::UpdateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .complete_promise(
                worker_id,
                oplog_id,
                data,
                worker_namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        Ok(value)
    }

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::UpdateWorker, auth)
            .await?;

        self.base_worker_service
            .interrupt(
                worker_id,
                recover_immediately,
                worker_namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        Ok(())
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth: &CloudAuthCtx,
    ) -> Result<crate::model::WorkerMetadata, WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::ViewWorker, auth)
            .await?;

        let metadata = self
            .base_worker_service
            .get_metadata(
                worker_id,
                worker_namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        let metadata = convert_metadata(metadata, worker_namespace.namespace.account_id);

        Ok(metadata)
    }

    async fn find_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        auth: &CloudAuthCtx,
    ) -> Result<(Option<ScanCursor>, Vec<crate::model::WorkerMetadata>), WorkerError> {
        let worker_namespace = self
            .authorize(component_id, &ProjectAction::ViewWorker, auth)
            .await?;

        let (pagination, metadata) = self
            .base_worker_service
            .find_metadata(
                component_id,
                filter,
                cursor,
                count,
                precise,
                worker_namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        let metadata = metadata
            .into_iter()
            .map(|metadata| {
                convert_metadata(metadata, worker_namespace.namespace.account_id.clone())
            })
            .collect();

        Ok((pagination, metadata))
    }

    async fn resume(&self, worker_id: &WorkerId, auth: &CloudAuthCtx) -> Result<(), WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::UpdateWorker, auth)
            .await?;
        let _ = self
            .base_worker_service
            .resume(
                worker_id,
                worker_namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        Ok(())
    }

    async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: UpdateMode,
        target_version: ComponentVersion,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::UpdateWorker, auth)
            .await?;

        let _ = self
            .base_worker_service
            .update(
                worker_id,
                update_mode,
                target_version,
                worker_namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        Ok(())
    }

    async fn get_component_for_worker(
        &self,
        worker_id: &WorkerId,
        auth: &CloudAuthCtx,
    ) -> Result<Component, WorkerError> {
        let worker_namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::ViewWorker, auth)
            .await?;

        let component = self
            .base_worker_service
            .get_component_for_worker(
                worker_id,
                worker_namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        Ok(component)
    }
}

fn convert_metadata(
    metadata: WorkerMetadata,
    account_id: AccountId,
) -> crate::model::WorkerMetadata {
    crate::model::WorkerMetadata {
        account_id,
        worker_id: metadata.worker_id,
        args: metadata.args,
        env: metadata.env,
        status: metadata.status,
        component_version: metadata.component_version,
        retry_count: metadata.retry_count,
        pending_invocation_count: metadata.pending_invocation_count,
        updates: metadata.updates,
        created_at: metadata.created_at,
        last_error: metadata.last_error,
        component_size: metadata.component_size,
        total_linear_memory_size: metadata.total_linear_memory_size,
        owned_resources: metadata.owned_resources,
    }
}

#[derive(Clone)]
pub struct WorkerNamespace {
    pub namespace: CloudNamespace,
    pub resource_limits: ResourceLimits,
}

impl WorkerNamespace {
    fn as_worker_request_metadata(&self) -> WorkerRequestMetadata {
        WorkerRequestMetadata {
            account_id: Some(self.namespace.account_id.clone()),
            limits: Some(self.resource_limits.clone()),
        }
    }
}
