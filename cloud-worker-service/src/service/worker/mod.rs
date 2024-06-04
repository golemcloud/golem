use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::{
    IdempotencyKey as ProtoIdempotencyKey, InvokeResult as ProtoInvokeResult, UpdateMode,
};
use golem_common::model::{
    AccountId, CallingConvention, ComponentId, ComponentVersion, IdempotencyKey, ProjectId,
    Timestamp, WorkerFilter, WorkerStatus,
};
use golem_wasm_rpc::protobuf::Val as ProtoVal;
use golem_worker_service_base::service::worker::{
    WorkerRequestMetadata, WorkerService as BaseWorkerService,
    WorkerServiceError as BaseWorkerServiceError,
};
use serde_json::Value;

use crate::service::auth::{AuthService, AuthServiceError, CloudAuthCtx};
use cloud_common::model::ProjectAction;
use golem_service_base::model::*;
use golem_wasm_rpc::TypeAnnotatedValue;

mod connect;

use crate::service::limit::{LimitError, LimitService};
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
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl From<AuthServiceError> for WorkerError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::Unauthorized(error) => WorkerError::Unauthorized(error),
            AuthServiceError::Forbidden(error) => WorkerError::Forbidden(error),
            AuthServiceError::Internal(error) => WorkerError::Internal(error),
        }
    }
}

impl From<LimitError> for WorkerError {
    fn from(error: LimitError) -> Self {
        match error {
            LimitError::Unauthorized(string) => WorkerError::Unauthorized(string),
            LimitError::LimitExceeded(string) => WorkerError::Forbidden(string),
            LimitError::Internal(e) => {
                WorkerError::Base(BaseWorkerServiceError::Internal(anyhow::Error::msg(e)))
            }
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

    async fn invoke_and_await_function(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Value,
        calling_convention: &CallingConvention,
        auth: &CloudAuthCtx,
    ) -> Result<Value, WorkerError>;

    async fn invoke_and_await_function_proto(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        calling_convention: &CallingConvention,
        auth: &CloudAuthCtx,
    ) -> Result<ProtoInvokeResult, WorkerError>;

    async fn invoke_and_await_function_typed_value(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Value,
        calling_convention: &CallingConvention,
        auth: &CloudAuthCtx,
    ) -> Result<TypeAnnotatedValue, WorkerError>;

    async fn invoke_function(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Value,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError>;

    async fn invoke_function_proto(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
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
        cursor: u64,
        count: u64,
        precise: bool,
        auth: &CloudAuthCtx,
    ) -> Result<(Option<u64>, Vec<crate::model::WorkerMetadata>), WorkerError>;

    async fn resume(&self, worker_id: &WorkerId, auth: &CloudAuthCtx) -> Result<(), WorkerError>;

    async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: UpdateMode,
        target_version: ComponentVersion,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError>;
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
            account_id: namespace.account_id.clone(),
            resource_limits,
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
        let namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .create(
                worker_id,
                component_version,
                arguments,
                environment_variables,
                namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        self.limit_service
            .update_worker_limit(&namespace.account_id, worker_id, 1)
            .await?;

        Ok(value)
    }

    async fn connect(
        &self,
        worker_id: &WorkerId,
        auth: &CloudAuthCtx,
    ) -> Result<ConnectWorkerStream, WorkerError> {
        let namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .connect(worker_id, namespace.as_worker_request_metadata(), auth)
            .await?;

        self.limit_service
            .update_worker_connection_limit(&namespace.account_id, worker_id, 1)
            .await?;

        Ok(ConnectWorkerStream::new(
            value,
            worker_id.clone(),
            namespace.account_id,
            self.limit_service.clone(),
        ))
    }

    async fn delete(&self, worker_id: &WorkerId, auth: &CloudAuthCtx) -> Result<(), WorkerError> {
        let namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::DeleteWorker, auth)
            .await?;

        self.base_worker_service.delete(worker_id, auth).await?;

        self.limit_service
            .update_worker_limit(&namespace.account_id, worker_id, -1)
            .await?;

        Ok(())
    }

    async fn invoke_and_await_function(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Value,
        calling_convention: &CallingConvention,
        auth: &CloudAuthCtx,
    ) -> Result<Value, WorkerError> {
        let namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .invoke_and_await_function(
                worker_id,
                idempotency_key,
                function_name,
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
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        calling_convention: &CallingConvention,
        auth: &CloudAuthCtx,
    ) -> Result<ProtoInvokeResult, WorkerError> {
        let namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .invoke_and_await_function_proto(
                worker_id,
                idempotency_key,
                function_name,
                params,
                calling_convention,
                namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        Ok(value)
    }

    async fn invoke_and_await_function_typed_value(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Value,
        calling_convention: &CallingConvention,
        auth: &CloudAuthCtx,
    ) -> Result<TypeAnnotatedValue, WorkerError> {
        let namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;

        let value = self
            .base_worker_service
            .invoke_and_await_function_typed_value(
                worker_id,
                idempotency_key,
                function_name,
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
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Value,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        let namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::CreateWorker, auth)
            .await?;
        let _ = self
            .base_worker_service
            .invoke_function(
                worker_id,
                idempotency_key,
                function_name,
                params,
                namespace.as_worker_request_metadata(),
                auth,
            )
            .await?;

        Ok(())
    }

    async fn invoke_function_proto(
        &self,
        worker_id: &WorkerId,
        idempotency_key: Option<ProtoIdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
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
        auth: &CloudAuthCtx,
    ) -> Result<bool, WorkerError> {
        let _ = self
            .authorize(&worker_id.component_id, &ProjectAction::UpdateWorker, auth)
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
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        let _ = self
            .authorize(&worker_id.component_id, &ProjectAction::UpdateWorker, auth)
            .await?;

        self.base_worker_service
            .interrupt(worker_id, recover_immediately, auth)
            .await?;

        Ok(())
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        auth: &CloudAuthCtx,
    ) -> Result<crate::model::WorkerMetadata, WorkerError> {
        let namespace = self
            .authorize(&worker_id.component_id, &ProjectAction::ViewWorker, auth)
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
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: u64,
        count: u64,
        precise: bool,
        auth: &CloudAuthCtx,
    ) -> Result<(Option<u64>, Vec<crate::model::WorkerMetadata>), WorkerError> {
        let namespace = self
            .authorize(component_id, &ProjectAction::ViewWorker, auth)
            .await?;

        let (pagination, metadata) = self
            .base_worker_service
            .find_metadata(component_id, filter, cursor, count, precise, auth)
            .await?;

        let metadata = metadata
            .into_iter()
            .map(|metadata| convert_metadata(metadata, namespace.account_id.clone()))
            .collect();

        Ok((pagination, metadata))
    }

    async fn resume(&self, worker_id: &WorkerId, auth: &CloudAuthCtx) -> Result<(), WorkerError> {
        let _ = self
            .authorize(&worker_id.component_id, &ProjectAction::UpdateWorker, auth)
            .await?;
        let _ = self.base_worker_service.resume(worker_id, auth).await?;

        Ok(())
    }

    async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: UpdateMode,
        target_version: ComponentVersion,
        auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        let _ = self
            .authorize(&worker_id.component_id, &ProjectAction::UpdateWorker, auth)
            .await?;
        let _ = self
            .base_worker_service
            .update(worker_id, update_mode, target_version, auth)
            .await?;
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
        component_version: metadata.component_version,
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
    pub resource_limits: ResourceLimits,
}

impl WorkerNamespace {
    fn as_worker_request_metadata(&self) -> WorkerRequestMetadata {
        WorkerRequestMetadata {
            account_id: Some(self.account_id.clone()),
            limits: Some(self.resource_limits.clone()),
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
        _component_version: u64,
        _arguments: Vec<String>,
        _environment_variables: HashMap<String, String>,
        _auth: &CloudAuthCtx,
    ) -> Result<WorkerId, WorkerError> {
        Ok(worker_id.clone())
    }

    async fn connect(
        &self,
        _worker_id: &WorkerId,
        _auth: &CloudAuthCtx,
    ) -> Result<ConnectWorkerStream, WorkerError> {
        Err(WorkerError::Base(BaseWorkerServiceError::Internal(
            anyhow::Error::msg("Not supported"),
        )))
    }

    async fn delete(&self, _worker_id: &WorkerId, _auth: &CloudAuthCtx) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn invoke_and_await_function(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: Option<IdempotencyKey>,
        _function_name: String,
        _params: Value,
        _calling_convention: &CallingConvention,
        _auth: &CloudAuthCtx,
    ) -> Result<Value, WorkerError> {
        Ok(Value::Null)
    }

    async fn invoke_and_await_function_proto(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: Option<ProtoIdempotencyKey>,
        _function_name: String,
        _params: Vec<ProtoVal>,
        _calling_convention: &CallingConvention,
        _auth: &CloudAuthCtx,
    ) -> Result<ProtoInvokeResult, WorkerError> {
        Ok(ProtoInvokeResult { result: vec![] })
    }

    async fn invoke_and_await_function_typed_value(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: Option<IdempotencyKey>,
        _function_name: String,
        _params: Value,
        _calling_convention: &CallingConvention,
        _auth: &CloudAuthCtx,
    ) -> Result<TypeAnnotatedValue, WorkerError> {
        Ok(TypeAnnotatedValue::Tuple {
            value: vec![],
            typ: vec![],
        })
    }

    async fn invoke_function(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: Option<IdempotencyKey>,
        _function_name: String,
        _params: Value,
        _auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn invoke_function_proto(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: Option<ProtoIdempotencyKey>,
        _function_name: String,
        _params: Vec<ProtoVal>,
        _auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn complete_promise(
        &self,
        _worker_id: &WorkerId,
        _oplog_id: u64,
        _data: Vec<u8>,
        _auth: &CloudAuthCtx,
    ) -> Result<bool, WorkerError> {
        Ok(true)
    }

    async fn interrupt(
        &self,
        _worker_id: &WorkerId,
        _recover_immediately: bool,
        _auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        _auth: &CloudAuthCtx,
    ) -> Result<crate::model::WorkerMetadata, WorkerError> {
        Ok(crate::model::WorkerMetadata {
            worker_id: worker_id.clone(),
            account_id: AccountId::from(""),
            args: vec![],
            env: Default::default(),
            status: WorkerStatus::Running,
            component_version: 0,
            retry_count: 0,
            created_at: Timestamp::now_utc(),
            pending_invocation_count: 0,
            updates: vec![],
            last_error: None,
        })
    }

    async fn find_metadata(
        &self,
        _component_id: &ComponentId,
        _filter: Option<WorkerFilter>,
        _cursor: u64,
        _count: u64,
        _precise: bool,
        _auth: &CloudAuthCtx,
    ) -> Result<(Option<u64>, Vec<crate::model::WorkerMetadata>), WorkerError> {
        Ok((None, vec![]))
    }

    async fn resume(&self, _worker_id: &WorkerId, _auth: &CloudAuthCtx) -> Result<(), WorkerError> {
        Ok(())
    }

    async fn update(
        &self,
        _worker_id: &WorkerId,
        _update_mode: UpdateMode,
        _target_version: ComponentVersion,
        _auth: &CloudAuthCtx,
    ) -> Result<(), WorkerError> {
        Ok(())
    }
}
