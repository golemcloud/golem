use async_trait::async_trait;
use bytes::Bytes;
use cloud_common::auth::CloudNamespace;
use cloud_common::clients::auth::AuthServiceError;
use cloud_common::clients::limit::{LimitError, LimitService};
use futures::Stream;
use golem_api_grpc::proto::golem::worker::{
    InvocationContext, InvokeResult as ProtoInvokeResult, UpdateMode,
};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::OplogCursor;
use golem_common::model::{
    AccountId, ComponentFilePath, ComponentFileSystemNode, ComponentId, ComponentVersion,
    IdempotencyKey, PluginInstallationId, ProjectId, ScanCursor, TargetWorkerId, WorkerFilter,
    WorkerId,
};
use golem_common::SafeDisplay;
use golem_service_base::model::*;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::Val as ProtoVal;
use golem_worker_service_base::service::worker::{
    WorkerRequestMetadata, WorkerResult, WorkerService as BaseWorkerService,
    WorkerServiceError as BaseWorkerServiceError,
};
use std::collections::HashMap;
use std::pin::Pin;
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
            LimitError::InternalClientError(message) => {
                WorkerError::Base(BaseWorkerServiceError::Internal(message))
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
        namespace: CloudNamespace,
    ) -> Result<WorkerId, WorkerError>;

    async fn connect(
        &self,
        worker_id: &WorkerId,
        namespace: CloudNamespace,
    ) -> Result<ConnectWorkerStream, WorkerError>;

    async fn delete(
        &self,
        worker_id: &WorkerId,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError>;

    #[allow(clippy::result_large_err)]
    fn validate_typed_parameters(
        &self,
        params: Vec<TypeAnnotatedValue>,
    ) -> Result<Vec<ProtoVal>, WorkerError>;

    /// Validates the provided list of `TypeAnnotatedValue` parameters, and then
    /// invokes the worker and waits its results, returning it as a `TypeAnnotatedValue`.
    async fn validate_and_invoke_and_await_typed(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<TypeAnnotatedValue>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<TypeAnnotatedValue, WorkerError> {
        let params = self.validate_typed_parameters(params)?;
        self.invoke_and_await_typed(
            worker_id,
            idempotency_key,
            function_name,
            params,
            invocation_context,
            namespace,
        )
        .await
    }

    /// Invokes a worker using raw `Val` parameter values and awaits its results returning
    /// it as a `TypeAnnotatedValue`.
    async fn invoke_and_await_typed(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<TypeAnnotatedValue, WorkerError>;

    /// Invokes a worker using raw `Val` parameter values and awaits its results returning
    /// a `Val` values (without type information)
    async fn invoke_and_await(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<ProtoInvokeResult, WorkerError>;

    /// Invokes a worker using JSON value encoding represented by raw strings and awaits its results
    /// returning it as a `TypeAnnotatedValue`. The input parameter JSONs cannot be converted to `Val`
    /// without type information so they get forwarded to the executor.
    async fn invoke_and_await_json(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<String>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<TypeAnnotatedValue, WorkerError>;

    /// Validates the provided list of `TypeAnnotatedValue` parameters, and then enqueues
    /// an invocation for the worker without awaiting its results.
    async fn validate_and_invoke(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<TypeAnnotatedValue>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError> {
        let params = self.validate_typed_parameters(params)?;
        self.invoke(
            worker_id,
            idempotency_key,
            function_name,
            params,
            invocation_context,
            namespace,
        )
        .await
    }

    /// Enqueues an invocation for the worker without awaiting its results, using raw `Val`
    /// parameters.
    async fn invoke(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError>;

    /// Enqueues an invocation for the worker without awaiting its results, using JSON value
    /// encoding represented as raw strings. Without type information these representations cannot
    /// be converted to `Val` so they get forwarded as-is to the executor.
    async fn invoke_json(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<String>,
        invocation_context: Option<InvocationContext>,
        metadata: CloudNamespace,
    ) -> Result<(), WorkerError>;

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        namespace: CloudNamespace,
    ) -> Result<bool, WorkerError>;

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError>;

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        namespace: CloudNamespace,
    ) -> Result<crate::model::WorkerMetadata, WorkerError>;

    async fn find_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        namespace: CloudNamespace,
    ) -> Result<(Option<ScanCursor>, Vec<crate::model::WorkerMetadata>), WorkerError>;

    async fn resume(
        &self,
        worker_id: &WorkerId,
        namespace: CloudNamespace,
        force: bool,
    ) -> Result<(), WorkerError>;

    async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: UpdateMode,
        target_version: ComponentVersion,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError>;

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        namespace: CloudNamespace,
    ) -> Result<GetOplogResponse, WorkerError>;

    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        cursor: Option<OplogCursor>,
        count: u64,
        query: String,
        namespace: CloudNamespace,
    ) -> Result<GetOplogResponse, WorkerError>;

    async fn list_directory(
        &self,
        worker_id: &TargetWorkerId,
        path: ComponentFilePath,
        namespace: CloudNamespace,
    ) -> Result<Vec<ComponentFileSystemNode>, WorkerError>;

    async fn get_file_contents(
        &self,
        worker_id: &TargetWorkerId,
        path: ComponentFilePath,
        namespace: CloudNamespace,
    ) -> Result<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>, WorkerError>;

    async fn activate_plugin(
        &self,
        worker_id: &WorkerId,
        plugin_installation_id: &PluginInstallationId,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError>;

    async fn deactivate_plugin(
        &self,
        worker_id: &WorkerId,
        plugin_installation_id: &PluginInstallationId,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError>;

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError>;

    async fn revert_worker(
        &self,
        worker_id: &WorkerId,
        target: RevertWorkerTarget,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError>;

    async fn cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        namespace: CloudNamespace,
    ) -> Result<bool, WorkerError>;
}

#[derive(Clone)]
pub struct WorkerServiceDefault {
    limit_service: Arc<dyn LimitService + Sync + Send>,
    base_worker_service: Arc<dyn BaseWorkerService + Send + Sync>,
}

impl WorkerServiceDefault {
    pub fn new(
        limit_service: Arc<dyn LimitService + Sync + Send>,
        base_worker_service: Arc<dyn BaseWorkerService + Send + Sync>,
    ) -> Self {
        WorkerServiceDefault {
            limit_service,
            base_worker_service,
        }
    }

    async fn authorize(&self, namespace: CloudNamespace) -> Result<WorkerNamespace, WorkerError> {
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
        namespace: CloudNamespace,
    ) -> Result<WorkerId, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;
        let account_id = worker_namespace.namespace.account_id.clone();

        let value = self
            .base_worker_service
            .create(
                worker_id,
                component_version,
                arguments,
                environment_variables,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        self.limit_service
            .update_worker_limit(&account_id, worker_id, 1)
            .await?;

        Ok(value)
    }

    async fn connect(
        &self,
        worker_id: &WorkerId,
        namespace: CloudNamespace,
    ) -> Result<ConnectWorkerStream, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        let value = self
            .base_worker_service
            .connect(
                worker_id,
                worker_namespace.clone().into_worker_request_metadata(),
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

    async fn delete(
        &self,
        worker_id: &WorkerId,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;
        let account_id = worker_namespace.namespace.account_id.clone();

        self.base_worker_service
            .delete(worker_id, worker_namespace.into_worker_request_metadata())
            .await?;

        self.limit_service
            .update_worker_limit(&account_id, worker_id, -1)
            .await?;

        Ok(())
    }

    fn validate_typed_parameters(
        &self,
        params: Vec<TypeAnnotatedValue>,
    ) -> Result<Vec<ProtoVal>, WorkerError> {
        let result = self.base_worker_service.validate_typed_parameters(params)?;
        Ok(result)
    }

    async fn invoke_and_await_typed(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<TypeAnnotatedValue, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        let result = self
            .base_worker_service
            .invoke_and_await_typed(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(result)
    }

    async fn invoke_and_await(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<ProtoInvokeResult, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        let result = self
            .base_worker_service
            .invoke_and_await(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(result)
    }

    async fn invoke_and_await_json(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<String>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<TypeAnnotatedValue, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        let result = self
            .base_worker_service
            .invoke_and_await_json(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(result)
    }

    async fn invoke(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<ProtoVal>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        self.base_worker_service
            .invoke(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn invoke_json(
        &self,
        worker_id: &TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function_name: String,
        params: Vec<String>,
        invocation_context: Option<InvocationContext>,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        self.base_worker_service
            .invoke_json(
                worker_id,
                idempotency_key,
                function_name,
                params,
                invocation_context,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn complete_promise(
        &self,
        worker_id: &WorkerId,
        oplog_id: u64,
        data: Vec<u8>,
        namespace: CloudNamespace,
    ) -> Result<bool, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        let value = self
            .base_worker_service
            .complete_promise(
                worker_id,
                oplog_id,
                data,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(value)
    }

    async fn interrupt(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        self.base_worker_service
            .interrupt(
                worker_id,
                recover_immediately,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn get_metadata(
        &self,
        worker_id: &WorkerId,
        namespace: CloudNamespace,
    ) -> Result<crate::model::WorkerMetadata, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;
        let account_id = worker_namespace.namespace.account_id.clone();

        let metadata = self
            .base_worker_service
            .get_metadata(worker_id, worker_namespace.into_worker_request_metadata())
            .await?;

        let metadata = convert_metadata(metadata, account_id);

        Ok(metadata)
    }

    async fn find_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        namespace: CloudNamespace,
    ) -> Result<(Option<ScanCursor>, Vec<crate::model::WorkerMetadata>), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;
        let account_id = worker_namespace.namespace.account_id.clone();

        let (pagination, metadata) = self
            .base_worker_service
            .find_metadata(
                component_id,
                filter,
                cursor,
                count,
                precise,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        let metadata = metadata
            .into_iter()
            .map(|metadata| convert_metadata(metadata, account_id.clone()))
            .collect();

        Ok((pagination, metadata))
    }

    async fn resume(
        &self,
        worker_id: &WorkerId,
        namespace: CloudNamespace,
        force: bool,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;
        let _ = self
            .base_worker_service
            .resume(
                worker_id,
                worker_namespace.into_worker_request_metadata(),
                force,
            )
            .await?;

        Ok(())
    }

    async fn update(
        &self,
        worker_id: &WorkerId,
        update_mode: UpdateMode,
        target_version: ComponentVersion,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        let _ = self
            .base_worker_service
            .update(
                worker_id,
                update_mode,
                target_version,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        namespace: CloudNamespace,
    ) -> Result<GetOplogResponse, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        let response = self
            .base_worker_service
            .get_oplog(
                worker_id,
                from_oplog_index,
                cursor,
                count,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(response)
    }

    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        cursor: Option<OplogCursor>,
        count: u64,
        query: String,
        namespace: CloudNamespace,
    ) -> Result<GetOplogResponse, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        let response = self
            .base_worker_service
            .search_oplog(
                worker_id,
                cursor,
                count,
                query,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(response)
    }

    async fn list_directory(
        &self,
        worker_id: &TargetWorkerId,
        path: ComponentFilePath,
        namespace: CloudNamespace,
    ) -> Result<Vec<ComponentFileSystemNode>, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        let response = self
            .base_worker_service
            .list_directory(
                worker_id,
                path,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(response)
    }

    async fn get_file_contents(
        &self,
        worker_id: &TargetWorkerId,
        path: ComponentFilePath,
        namespace: CloudNamespace,
    ) -> Result<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>, WorkerError>
    {
        let worker_namespace = self.authorize(namespace).await?;

        let response = self
            .base_worker_service
            .get_file_contents(
                worker_id,
                path,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(response)
    }

    async fn activate_plugin(
        &self,
        worker_id: &WorkerId,
        plugin_installation_id: &PluginInstallationId,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        self.base_worker_service
            .activate_plugin(
                worker_id,
                plugin_installation_id,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn deactivate_plugin(
        &self,
        worker_id: &WorkerId,
        plugin_installation_id: &PluginInstallationId,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        self.base_worker_service
            .deactivate_plugin(
                worker_id,
                plugin_installation_id,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        self.base_worker_service
            .fork_worker(
                source_worker_id,
                target_worker_id,
                oplog_index_cut_off,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn revert_worker(
        &self,
        worker_id: &WorkerId,
        target: RevertWorkerTarget,
        namespace: CloudNamespace,
    ) -> Result<(), WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        self.base_worker_service
            .revert_worker(
                worker_id,
                target,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(())
    }

    async fn cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        namespace: CloudNamespace,
    ) -> Result<bool, WorkerError> {
        let worker_namespace = self.authorize(namespace).await?;

        let result = self
            .base_worker_service
            .cancel_invocation(
                worker_id,
                idempotency_key,
                worker_namespace.into_worker_request_metadata(),
            )
            .await?;

        Ok(result)
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
        active_plugins: metadata.active_plugins,
    }
}

#[derive(Clone)]
pub struct WorkerNamespace {
    pub namespace: CloudNamespace,
    pub resource_limits: ResourceLimits,
}

impl WorkerNamespace {
    fn into_worker_request_metadata(self) -> WorkerRequestMetadata {
        WorkerRequestMetadata {
            account_id: Some(self.namespace.account_id),
            limits: Some(self.resource_limits),
        }
    }
}
