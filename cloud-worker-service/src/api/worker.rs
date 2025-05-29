use std::sync::Arc;
use std::time::Duration;

use crate::api::common::ApiTags;
use crate::model;
use crate::service::auth::AuthService;
use crate::service::worker::{
    ConnectWorkerStream, WorkerError as WorkerServiceError, WorkerService,
};
use cloud_common::auth::{
    CloudAuthCtx, CloudNamespace, GolemSecurityScheme, WrappedGolemSecuritySchema,
};
use cloud_common::clients::auth::AuthServiceError;
use cloud_common::model::{ProjectAction, TokenSecret};
use futures::StreamExt;
use futures_util::TryStreamExt;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::error::{
    ErrorBody, ErrorsBody, GolemError, GolemErrorBody, GolemErrorUnknown,
};
use golem_common::model::oplog::OplogIndex;
use golem_common::model::public_oplog::OplogCursor;
use golem_common::model::{
    ComponentFilePath, ComponentId, IdempotencyKey, PluginInstallationId, ScanCursor,
    TargetWorkerId, WorkerFilter, WorkerId,
};
use golem_common::{recorded_http_api_request, SafeDisplay};
use golem_service_base::model::*;
use golem_worker_service_base::service::component::{ComponentService, ComponentServiceError};
use golem_worker_service_base::service::worker::{proxy_worker_connection, InvocationParameters};
use poem::web::websocket::{BoxWebSocketUpgraded, WebSocket};
use poem::Body;
use poem_openapi::param::{Header, Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::*;
use std::str::FromStr;
use tap::TapFallible;
use tonic::Status;
use tracing::Instrument;

const WORKER_CONNECT_PING_INTERVAL: Duration = Duration::from_secs(30);
const WORKER_CONNECT_PING_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(ApiResponse, Debug, Clone)]
pub enum WorkerError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Maximum number of workers exceeded
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    /// Component / Worker / Promise not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    /// Worker already exists
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<GolemErrorBody>),
}

impl TraceErrorKind for WorkerError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            WorkerError::BadRequest(_) => "BadRequest",
            WorkerError::NotFound(_) => "NotFound",
            WorkerError::AlreadyExists(_) => "AlreadyExists",
            WorkerError::LimitExceeded(_) => "LimitExceeded",
            WorkerError::Unauthorized(_) => "Unauthorized",
            WorkerError::InternalError(_) => "InternalError",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            WorkerError::BadRequest(_) => true,
            WorkerError::NotFound(_) => true,
            WorkerError::AlreadyExists(_) => true,
            WorkerError::LimitExceeded(_) => true,
            WorkerError::Unauthorized(_) => true,
            WorkerError::InternalError(_) => false,
        }
    }
}

impl WorkerError {
    fn bad_request(error: String) -> WorkerError {
        WorkerError::BadRequest(Json(ErrorsBody {
            errors: vec![error],
        }))
    }
}

type Result<T> = std::result::Result<T, WorkerError>;

impl From<tonic::transport::Error> for WorkerError {
    fn from(value: tonic::transport::Error) -> Self {
        WorkerError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown {
                details: value.to_string(),
            }),
        }))
    }
}

impl From<Status> for WorkerError {
    fn from(value: Status) -> Self {
        WorkerError::InternalError(Json(GolemErrorBody {
            golem_error: GolemError::Unknown(GolemErrorUnknown {
                details: value.to_string(),
            }),
        }))
    }
}

impl From<ComponentServiceError> for WorkerError {
    fn from(value: ComponentServiceError) -> Self {
        match value {
            ComponentServiceError::BadRequest(errors) => {
                WorkerError::BadRequest(Json(ErrorsBody { errors }))
            }
            ComponentServiceError::AlreadyExists(error) => {
                WorkerError::AlreadyExists(Json(ErrorBody { error }))
            }
            ComponentServiceError::Internal(error) => {
                WorkerError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown {
                        details: error.to_string(),
                    }),
                }))
            }
            ComponentServiceError::Unauthorized(error) => {
                WorkerError::Unauthorized(Json(ErrorBody { error }))
            }
            ComponentServiceError::Forbidden(error) => {
                WorkerError::LimitExceeded(Json(ErrorBody { error }))
            }
            ComponentServiceError::NotFound(error) => {
                WorkerError::NotFound(Json(ErrorBody { error }))
            }
            ComponentServiceError::FailedGrpcStatus(_) => {
                WorkerError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown {
                        details: value.to_safe_string(),
                    }),
                }))
            }
            ComponentServiceError::FailedTransport(_) => {
                WorkerError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown {
                        details: value.to_safe_string(),
                    }),
                }))
            }
        }
    }
}

impl From<WorkerServiceError> for WorkerError {
    fn from(value: WorkerServiceError) -> Self {
        use golem_worker_service_base::service::worker::WorkerServiceError as BaseServiceError;

        match value {
            WorkerServiceError::Forbidden(error) => WorkerError::LimitExceeded(Json(ErrorBody {
                error: error.clone(),
            })),
            WorkerServiceError::Unauthorized(error) => WorkerError::Unauthorized(Json(ErrorBody {
                error: error.clone(),
            })),
            WorkerServiceError::ProjectNotFound(_) => WorkerError::NotFound(Json(ErrorBody {
                error: value.to_string(),
            })),
            WorkerServiceError::Base(error) => match error {
                BaseServiceError::VersionedComponentIdNotFound(_)
                | BaseServiceError::ComponentNotFound(_)
                | BaseServiceError::AccountIdNotFound(_)
                | BaseServiceError::WorkerNotFound(_) => WorkerError::NotFound(Json(ErrorBody {
                    error: error.to_string(),
                })),
                BaseServiceError::TypeChecker(error) => WorkerError::bad_request(error.clone()),
                BaseServiceError::Component(error) => error.into(),
                BaseServiceError::Internal(error) => {
                    WorkerError::InternalError(Json(GolemErrorBody {
                        golem_error: GolemError::Unknown(GolemErrorUnknown {
                            details: error.to_string(),
                        }),
                    }))
                }
                BaseServiceError::Golem(golem_error) => match golem_error {
                    GolemError::WorkerNotFound(error) => WorkerError::NotFound(Json(ErrorBody {
                        error: error.to_safe_string(),
                    })),
                    _ => WorkerError::InternalError(Json(GolemErrorBody {
                        golem_error: golem_error.clone(),
                    })),
                },
                BaseServiceError::InternalCallError(inner) => {
                    WorkerError::InternalError(Json(GolemErrorBody {
                        golem_error: GolemError::Unknown(GolemErrorUnknown {
                            details: inner.to_safe_string(),
                        }),
                    }))
                }
                BaseServiceError::FileNotFound(_) => WorkerError::BadRequest(Json(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                })),
                BaseServiceError::BadFileType(_) => WorkerError::BadRequest(Json(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                })),
            },
            WorkerServiceError::InternalAuthServiceError(_) => {
                WorkerError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown {
                        details: value.to_safe_string(),
                    }),
                }))
            }
        }
    }
}

impl From<AuthServiceError> for WorkerError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::Unauthorized(error) => {
                WorkerError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Forbidden(error) => {
                WorkerError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::InternalClientError(error) => {
                WorkerError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown { details: error }),
                }))
            }
        }
    }
}

pub struct WorkerApi {
    component_service: Arc<dyn ComponentService<CloudNamespace, CloudAuthCtx>>,
    worker_service: Arc<dyn WorkerService + Send + Sync>,
    worker_auth_service: Arc<dyn AuthService + Send + Sync>,
}

#[OpenApi(prefix_path = "/v1/components", tag = ApiTags::Worker)]
impl WorkerApi {
    pub fn new(
        component_service: Arc<dyn ComponentService<CloudNamespace, CloudAuthCtx>>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        auth_service: Arc<dyn AuthService + Send + Sync>,
    ) -> Self {
        Self {
            component_service,
            worker_service,
            worker_auth_service: auth_service,
        }
    }

    /// Launch a new worker.
    ///
    /// Creates a new worker. The worker initially is in `Idle`` status, waiting to be invoked.
    ///
    /// The parameters in the request are the following:
    /// - `name` is the name of the created worker. This has to be unique, but only for a given component
    /// - `args` is a list of strings which appear as command line arguments for the worker
    /// - `env` is a list of key-value pairs (represented by arrays) which appear as environment variables for the worker
    #[oai(
        path = "/:component_id/workers",
        method = "post",
        operation_id = "launch_new_worker"
    )]
    async fn launch_new_worker(
        &self,
        component_id: Path<ComponentId>,
        request: Json<WorkerCreationRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<WorkerCreationResponse>> {
        let record = recorded_http_api_request!(
            "launch_new_worker",
            component_id = component_id.0.to_string(),
            name = request.name
        );

        let response = self
            .launch_new_worker_internal(component_id.0, request.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn launch_new_worker_internal(
        &self,
        component_id: ComponentId,
        request: WorkerCreationRequest,
        token: GolemSecurityScheme,
    ) -> Result<Json<WorkerCreationResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let latest_component = self
            .component_service
            .get_latest(&component_id, &auth)
            .await
            .tap_err(|error| tracing::error!("Error getting latest component: {:?}", error))
            .map_err(|error| {
                WorkerError::NotFound(Json(ErrorBody {
                    error: format!(
                        "Couldn't retrieve the component: {}. error: {}",
                        &component_id, error
                    ),
                }))
            })?;

        let WorkerCreationRequest { name, args, env } = request;

        let worker_id = validated_worker_id(component_id, name)?;

        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::CreateWorker, &auth)
            .await?;

        let _worker = self
            .worker_service
            .create(
                &worker_id,
                latest_component.versioned_component_id.version,
                args,
                env,
                namespace,
            )
            .await?;

        Ok(Json(WorkerCreationResponse {
            worker_id,
            component_version: latest_component.versioned_component_id.version,
        }))
    }

    /// Delete a worker
    ///
    /// Interrupts and deletes an existing worker.
    #[oai(
        path = "/:component_id/workers/:worker_name",
        method = "delete",
        operation_id = "delete_worker"
    )]
    async fn delete_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeleteWorkerResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record =
            recorded_http_api_request!("delete_worker", worker_id = worker_id.to_string(),);

        let response = self
            .delete_worker_internal(worker_id, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_worker_internal(
        &self,
        worker_id: WorkerId,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeleteWorkerResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::DeleteWorker, &auth)
            .await?;
        self.worker_service.delete(&worker_id, namespace).await?;
        Ok(Json(DeleteWorkerResponse {}))
    }

    /// Invoke a function and await its resolution
    ///
    /// Supply the parameters in the request body as JSON.
    #[oai(
        path = "/:component_id/workers/:worker_name/invoke-and-await",
        method = "post",
        operation_id = "invoke_and_await_function"
    )]
    async fn invoke_and_await_function(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        function: Query<String>,
        params: Json<InvokeParameters>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvokeResult>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!(
            "invoke_and_await_function",
            worker_id = worker_id.to_string(),
            idempotency_key = idempotency_key.0.as_ref().map(|v| v.value.clone()),
            function = function.0
        );

        let response = self
            .invoke_and_await_function_internal(
                worker_id.into_target_worker_id(),
                idempotency_key.0,
                function.0,
                params.0,
                token,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn invoke_and_await_function_internal(
        &self,
        target_worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        params: InvokeParameters,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvokeResult>> {
        let auth = CloudAuthCtx::new(token.secret());

        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(
                &target_worker_id.component_id,
                ProjectAction::UpdateWorker,
                &auth,
            )
            .await?;

        let params =
            InvocationParameters::from_optionally_type_annotated_value_jsons(params.params)
                .map_err(|errors| WorkerError::BadRequest(Json(ErrorsBody { errors })))?;

        let result = match params {
            InvocationParameters::TypedProtoVals(vals) => {
                self.worker_service.validate_and_invoke_and_await_typed(
                    &target_worker_id,
                    idempotency_key,
                    function,
                    vals,
                    None,
                    namespace,
                )
            }
            InvocationParameters::RawJsonStrings(jsons) => {
                self.worker_service.invoke_and_await_json(
                    &target_worker_id,
                    idempotency_key,
                    function,
                    jsons,
                    None,
                    namespace,
                )
            }
        }
        .await?;

        Ok(Json(InvokeResult { result }))
    }

    /// Invoke a function and await its resolution on a new worker with a random generated name
    ///
    /// Ideal for invoking ephemeral components, but works with durable ones as well.
    /// Supply the parameters in the request body as JSON.
    #[oai(
        path = "/:component_id/invoke-and-await",
        method = "post",
        operation_id = "invoke_and_await_function_without_name"
    )]
    async fn invoke_and_await_function_without_name(
        &self,
        component_id: Path<ComponentId>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        function: Query<String>,
        params: Json<InvokeParameters>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvokeResult>> {
        let target_worker_id = make_target_worker_id(component_id.0, None)?;

        let record = recorded_http_api_request!(
            "invoke_and_await_function_without_name",
            worker_id = target_worker_id.to_string(),
            idempotency_key = idempotency_key.0.as_ref().map(|v| v.value.clone()),
            function = function.0
        );

        let response = self
            .invoke_and_await_function_internal(
                target_worker_id,
                idempotency_key.0,
                function.0,
                params.0,
                token,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    /// Invoke a function
    ///
    /// A simpler version of the previously defined invoke and await endpoint just triggers the execution of a function and immediately returns.
    #[oai(
        path = "/:component_id/workers/:worker_name/invoke",
        method = "post",
        operation_id = "invoke_function"
    )]
    async fn invoke_function(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        /// name of the exported function to be invoked
        function: Query<String>,
        params: Json<InvokeParameters>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvokeResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!(
            "invoke_function",
            worker_id = worker_id.to_string(),
            idempotency_key = idempotency_key.0.as_ref().map(|v| v.value.clone()),
            function = function.0
        );

        let response = self
            .invoke_function_internal(
                worker_id.into_target_worker_id(),
                idempotency_key.0,
                function.0,
                params.0,
                token,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn invoke_function_internal(
        &self,
        target_worker_id: TargetWorkerId,
        idempotency_key: Option<IdempotencyKey>,
        function: String,
        params: InvokeParameters,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvokeResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(
                &target_worker_id.component_id,
                ProjectAction::UpdateWorker,
                &auth,
            )
            .await?;

        let params =
            InvocationParameters::from_optionally_type_annotated_value_jsons(params.params)
                .map_err(|errors| WorkerError::BadRequest(Json(ErrorsBody { errors })))?;

        match params {
            InvocationParameters::TypedProtoVals(vals) => self.worker_service.validate_and_invoke(
                &target_worker_id,
                idempotency_key,
                function,
                vals,
                None,
                namespace,
            ),
            InvocationParameters::RawJsonStrings(jsons) => self.worker_service.invoke_json(
                &target_worker_id,
                idempotency_key,
                function,
                jsons,
                None,
                namespace,
            ),
        }
        .await?;
        Ok(Json(InvokeResponse {}))
    }

    /// Invoke a function on a new worker with a random generated name
    ///
    /// Ideal for invoking ephemeral components, but works with durable ones as well.
    /// A simpler version of the previously defined invoke and await endpoint just triggers the execution of a function and immediately returns.
    #[oai(
        path = "/:component_id/invoke",
        method = "post",
        operation_id = "invoke_function_without_name"
    )]
    async fn invoke_function_without_name(
        &self,
        component_id: Path<ComponentId>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<Option<IdempotencyKey>>,
        /// name of the exported function to be invoked
        function: Query<String>,
        params: Json<InvokeParameters>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvokeResponse>> {
        let target_worker_id = make_target_worker_id(component_id.0, None)?;

        let record = recorded_http_api_request!(
            "invoke_function_without_name",
            worker_id = target_worker_id.to_string(),
            idempotency_key = idempotency_key.0.as_ref().map(|v| v.value.clone()),
            function = function.0
        );

        let response = self
            .invoke_function_internal(
                target_worker_id,
                idempotency_key.0,
                function.0,
                params.0,
                token,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    /// Complete a promise
    ///
    /// Completes a promise with a given custom array of bytes.
    /// The promise must be previously created from within the worker, and it's identifier (a combination of a worker identifier and an oplogIdx ) must be sent out to an external caller so it can use this endpoint to mark the promise completed.
    /// The data field is sent back to the worker, and it has no predefined meaning.
    #[oai(
        path = "/:component_id/workers/:worker_name/complete",
        method = "post",
        operation_id = "complete_promise"
    )]
    async fn complete_promise(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        params: Json<CompleteParameters>,
        token: GolemSecurityScheme,
    ) -> Result<Json<bool>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record =
            recorded_http_api_request!("complete_promise", worker_id = worker_id.to_string());

        let response = self
            .complete_promise_internal(worker_id, params.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn complete_promise_internal(
        &self,
        worker_id: WorkerId,
        params: CompleteParameters,
        token: GolemSecurityScheme,
    ) -> Result<Json<bool>> {
        let auth = CloudAuthCtx::new(token.secret());
        let CompleteParameters { oplog_idx, data } = params;

        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        let response = self
            .worker_service
            .complete_promise(&worker_id, oplog_idx, data, namespace)
            .await?;

        Ok(Json(response))
    }

    /// Interrupt a worker
    ///
    /// Interrupts the execution of a worker.
    /// The worker's status will be Interrupted unless the recover-immediately parameter was used, in which case it remains as it was.
    /// An interrupted worker can be still used, and it is going to be automatically resumed the first time it is used.
    /// For example in case of a new invocation, the previously interrupted invocation is continued before the new one gets processed.
    #[oai(
        path = "/:component_id/workers/:worker_name/interrupt",
        method = "post",
        operation_id = "interrupt_worker"
    )]
    async fn interrupt_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        /// if true will simulate a worker recovery. Defaults to false.
        #[oai(name = "recovery-immediately")]
        recover_immediately: Query<Option<bool>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InterruptResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record =
            recorded_http_api_request!("interrupt_worker", worker_id = worker_id.to_string());

        let response = self
            .interrupt_worker_internal(worker_id, recover_immediately.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn interrupt_worker_internal(
        &self,
        worker_id: WorkerId,
        recover_immediately: Option<bool>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InterruptResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        self.worker_service
            .interrupt(&worker_id, recover_immediately.unwrap_or(false), namespace)
            .await?;

        Ok(Json(InterruptResponse {}))
    }

    /// Get metadata of a worker
    ///
    /// Returns metadata about an existing worker:
    /// - `workerId` is a combination of the used component and the worker's user specified name
    /// - `accountId` the account the worker is created by
    /// - `args` is the provided command line arguments passed to the worker
    /// - `env` is the provided map of environment variables passed to the worker
    /// - `componentVersion` is the version of the component used by the worker
    /// - `retryCount` is the number of retries the worker did in case of a failure
    /// - `status` is the worker's current status, one of the following:
    ///     - `Running` if the worker is currently executing
    ///     - `Idle` if the worker is waiting for an invocation
    ///     - `Suspended` if the worker was running but is now waiting to be resumed by an event (such as end of a sleep, a promise, etc)
    ///     - `Interrupted` if the worker was interrupted by the user
    ///     - `Retrying` if the worker failed, and an automatic retry was scheduled for it
    ///     - `Failed` if the worker failed and there are no more retries scheduled for it
    ///     - `Exited` if the worker explicitly exited using the exit WASI function
    #[oai(
        path = "/:component_id/workers/:worker_name",
        method = "get",
        operation_id = "get_worker_metadata"
    )]
    async fn get_worker_metadata(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<model::WorkerMetadata>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record =
            recorded_http_api_request!("get_worker_metadata", worker_id = worker_id.to_string());

        let response = self
            .get_worker_metadata_internal(worker_id, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_worker_metadata_internal(
        &self,
        worker_id: WorkerId,
        token: GolemSecurityScheme,
    ) -> Result<Json<model::WorkerMetadata>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await?;
        let response = self
            .worker_service
            .get_metadata(&worker_id, namespace)
            .await?;

        Ok(Json(response))
    }

    /// Get metadata of multiple workers
    ///
    /// ### Filters
    ///
    /// | Property    | Comparator             | Description                    | Example                         |
    /// |-------------|------------------------|--------------------------------|----------------------------------|
    /// | name        | StringFilterComparator | Name of worker                 | `name = worker-name`             |
    /// | version     | FilterComparator       | Version of worker              | `version >= 0`                   |
    /// | status      | FilterComparator       | Status of worker               | `status = Running`               |
    /// | env.\[key\] | StringFilterComparator | Environment variable of worker | `env.var1 = value`               |
    /// | createdAt   | FilterComparator       | Creation time of worker        | `createdAt > 2024-04-01T12:10:00Z` |
    ///
    ///
    /// ### Comparators
    ///
    /// - StringFilterComparator: `eq|equal|=|==`, `ne|notequal|!=`, `like`, `notlike`
    /// - FilterComparator: `eq|equal|=|==`, `ne|notequal|!=`, `ge|greaterequal|>=`, `gt|greater|>`, `le|lessequal|<=`, `lt|less|<`
    ///
    /// Returns metadata about an existing component workers:
    /// - `workers` list of workers metadata
    /// - `cursor` cursor for next request, if cursor is empty/null, there are no other values
    #[oai(
        path = "/:component_id/workers",
        method = "get",
        operation_id = "get_workers_metadata"
    )]
    async fn get_workers_metadata(
        &self,
        component_id: Path<ComponentId>,
        /// Filter for worker metadata in form of `property op value`. Can be used multiple times (AND condition is applied between them)
        filter: Query<Option<Vec<String>>>,
        /// Count of listed values, default: 50
        cursor: Query<Option<String>>,
        /// Position where to start listing, if not provided, starts from the beginning. It is used to get the next page of results. To get next page, use the cursor returned in the response
        count: Query<Option<u64>>,
        /// Precision in relation to worker status, if true, calculate the most up-to-date status for each worker, default is false
        precise: Query<Option<bool>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<model::WorkersMetadataResponse>> {
        let record = recorded_http_api_request!(
            "get_workers_metadata",
            component_id = component_id.0.to_string()
        );

        let response = self
            .get_workers_metadata_internal(
                component_id.0,
                filter.0,
                cursor.0,
                count.0,
                precise.0,
                token,
            )
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_workers_metadata_internal(
        &self,
        component_id: ComponentId,
        filter: Option<Vec<String>>,
        cursor: Option<String>,
        count: Option<u64>,
        precise: Option<bool>,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::WorkersMetadataResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&component_id, ProjectAction::ViewWorker, &auth)
            .await?;

        let filter = match filter {
            Some(filters) if !filters.is_empty() => Some(
                WorkerFilter::from(filters)
                    .map_err(|e| WorkerError::BadRequest(Json(ErrorsBody { errors: vec![e] })))?,
            ),
            _ => None,
        };

        let cursor = match cursor {
            Some(cursor) => Some(
                ScanCursor::from_str(&cursor)
                    .map_err(|e| WorkerError::BadRequest(Json(ErrorsBody { errors: vec![e] })))?,
            ),
            None => None,
        };

        let (cursor, workers) = self
            .worker_service
            .find_metadata(
                &component_id,
                filter,
                cursor.unwrap_or_default(),
                count.unwrap_or(50),
                precise.unwrap_or(false),
                namespace,
            )
            .await?;

        Ok(Json(crate::model::WorkersMetadataResponse {
            workers,
            cursor,
        }))
    }

    /// Advanced search for workers
    ///
    /// ### Filter types
    /// | Type      | Comparator             | Description                    | Example                                                                                       |
    /// |-----------|------------------------|--------------------------------|-----------------------------------------------------------------------------------------------|
    /// | Name      | StringFilterComparator | Name of worker                 | `{ "type": "Name", "comparator": "Equal", "value": "worker-name" }`                           |
    /// | Version   | FilterComparator       | Version of worker              | `{ "type": "Version", "comparator": "GreaterEqual", "value": 0 }`                             |
    /// | Status    | FilterComparator       | Status of worker               | `{ "type": "Status", "comparator": "Equal", "value": "Running" }`                             |
    /// | Env       | StringFilterComparator | Environment variable of worker | `{ "type": "Env", "name": "var1", "comparator": "Equal", "value": "value" }`                  |
    /// | CreatedAt | FilterComparator       | Creation time of worker        | `{ "type": "CreatedAt", "comparator": "Greater", "value": "2024-04-01T12:10:00Z" }`           |
    /// | And       |                        | And filter combinator          | `{ "type": "And", "filters": [ ... ] }`                                                       |
    /// | Or        |                        | Or filter combinator           | `{ "type": "Or", "filters": [ ... ] }`                                                        |
    /// | Not       |                        | Negates the specified filter   | `{ "type": "Not", "filter": { "type": "Version", "comparator": "GreaterEqual", "value": 0 } }`|
    ///
    /// ### Comparators
    /// - StringFilterComparator: `Equal`, `NotEqual`, `Like`, `NotLike`
    /// - FilterComparator: `Equal`, `NotEqual`, `GreaterEqual`, `Greater`, `LessEqual`, `Less`
    ///
    /// Returns metadata about an existing component workers:
    /// - `workers` list of workers metadata
    /// - `cursor` cursor for next request, if cursor is empty/null, there are no other values
    #[oai(
        path = "/:component_id/workers/find",
        method = "post",
        operation_id = "find_workers_metadata"
    )]
    async fn find_workers_metadata(
        &self,
        component_id: Path<ComponentId>,
        params: Json<WorkersMetadataRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::WorkersMetadataResponse>> {
        let record = recorded_http_api_request!(
            "find_workers_metadata",
            component_id = component_id.0.to_string()
        );

        let response = self
            .find_workers_metadata_internal(component_id.0, params.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn find_workers_metadata_internal(
        &self,
        component_id: ComponentId,
        params: WorkersMetadataRequest,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::WorkersMetadataResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&component_id, ProjectAction::ViewWorker, &auth)
            .await?;
        let (cursor, workers) = self
            .worker_service
            .find_metadata(
                &component_id,
                params.filter.clone(),
                params.cursor.clone().unwrap_or_default(),
                params.count.unwrap_or(50),
                params.precise.unwrap_or(false),
                namespace,
            )
            .await?;

        Ok(Json(crate::model::WorkersMetadataResponse {
            workers,
            cursor,
        }))
    }

    /// Resume a worker
    #[oai(
        path = "/:component_id/workers/:worker_name/resume",
        method = "post",
        operation_id = "resume_worker"
    )]
    async fn resume_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ResumeResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!("resume_worker", worker_id = worker_id.to_string());

        let response = self
            .resume_worker_internal(worker_id, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn resume_worker_internal(
        &self,
        worker_id: WorkerId,
        token: GolemSecurityScheme,
    ) -> Result<Json<ResumeResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        self.worker_service
            .resume(&worker_id, namespace, false)
            .await?;

        Ok(Json(ResumeResponse {}))
    }

    /// Update a worker
    #[oai(
        path = "/:component_id/workers/:worker_name/update",
        method = "post",
        operation_id = "update_worker"
    )]
    async fn update_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        params: Json<UpdateWorkerRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<UpdateWorkerResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!("update_worker", worker_id = worker_id.to_string());

        let response = self
            .update_worker_internal(worker_id, params.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_worker_internal(
        &self,
        worker_id: WorkerId,
        params: UpdateWorkerRequest,
        token: GolemSecurityScheme,
    ) -> Result<Json<UpdateWorkerResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;
        self.worker_service
            .update(
                &worker_id,
                params.mode.clone().into(),
                params.target_version,
                namespace,
            )
            .await?;

        Ok(Json(UpdateWorkerResponse {}))
    }

    /// Get the oplog of a worker
    #[oai(
        path = "/:component_id/workers/:worker_name/oplog",
        method = "get",
        operation_id = "get_oplog"
    )]
    async fn get_oplog(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        from: Query<Option<u64>>,
        count: Query<u64>,
        cursor: Query<Option<OplogCursor>>,
        query: Query<Option<String>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<GetOplogResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!("get_oplog", worker_id = worker_id.to_string());

        let response = self
            .get_oplog_internal(worker_id, from.0, count.0, cursor.0, query.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_oplog_internal(
        &self,
        worker_id: WorkerId,
        from: Option<u64>,
        count: u64,
        cursor: Option<OplogCursor>,
        query: Option<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<GetOplogResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await?;

        match (from, query) {
            (Some(_), Some(_)) => Err(WorkerError::BadRequest(Json(ErrorsBody {
                errors: vec![
                    "Cannot specify both the 'from' and the 'query' parameters".to_string()
                ],
            }))),
            (Some(from), None) => {
                let response = self
                    .worker_service
                    .get_oplog(
                        &worker_id,
                        OplogIndex::from_u64(from),
                        cursor,
                        count,
                        namespace,
                    )
                    .await?;

                Ok(Json(response))
            }
            (None, Some(query)) => {
                let response = self
                    .worker_service
                    .search_oplog(&worker_id, cursor, count, query, namespace)
                    .await?;

                Ok(Json(response))
            }
            (None, None) => {
                let response = self
                    .worker_service
                    .get_oplog(&worker_id, OplogIndex::INITIAL, cursor, count, namespace)
                    .await?;

                Ok(Json(response))
            }
        }
    }

    /// List files in a worker
    #[oai(
        path = "/:component_id/workers/:worker_name/files/:file_name",
        method = "get",
        operation_id = "get_files"
    )]
    async fn get_file(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        file_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<GetFilesResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;
        let record = recorded_http_api_request!("get_file", worker_id = worker_id.to_string());

        let response = self
            .get_file_internal(worker_id, file_name.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_file_internal(
        &self,
        worker_id: WorkerId,
        file_name: String,
        token: GolemSecurityScheme,
    ) -> Result<Json<GetFilesResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let path = make_component_file_path(file_name)?;

        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await?;

        let nodes = self
            .worker_service
            .list_directory(&worker_id.into_target_worker_id(), path, namespace)
            .await?;

        Ok(Json(GetFilesResponse {
            nodes: nodes.into_iter().map(|n| n.into()).collect(),
        }))
    }

    /// Get contents of a file in a worker
    #[oai(
        path = "/:component_id/workers/:worker_name/file-contents/:file_name",
        method = "get",
        operation_id = "get_file_content"
    )]
    async fn get_file_content(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        file_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Binary<Body>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;
        let record = recorded_http_api_request!("get_files", worker_id = worker_id.to_string());

        let response = self
            .get_file_content_internal(worker_id, file_name.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_file_content_internal(
        &self,
        worker_id: WorkerId,
        file_name: String,
        token: GolemSecurityScheme,
    ) -> Result<Binary<Body>> {
        let auth = CloudAuthCtx::new(token.secret());
        let path = make_component_file_path(file_name)?;

        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await?;

        let bytes = self
            .worker_service
            .get_file_contents(&worker_id.into_target_worker_id(), path, namespace)
            .await?;

        Ok(Binary(Body::from_bytes_stream(
            bytes.map_err(|e| std::io::Error::other(e.to_string())),
        )))
    }

    /// Activate a plugin
    ///
    /// The plugin must be one of the installed plugins for the worker's current component version.
    #[oai(
        path = "/:component_id/workers/:worker_name/activate-plugin",
        method = "post",
        operation_id = "activate_plugin"
    )]
    async fn activate_plugin(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        #[oai(name = "plugin-installation-id")] plugin_installation_id: Query<PluginInstallationId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ActivatePluginResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!(
            "activate_plugin",
            worker_id = worker_id.to_string(),
            plugin_installation_id = plugin_installation_id.to_string()
        );

        let response = self
            .activate_plugin_internal(worker_id, plugin_installation_id.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn activate_plugin_internal(
        &self,
        worker_id: WorkerId,
        plugin_installation_id: PluginInstallationId,
        token: GolemSecurityScheme,
    ) -> Result<Json<ActivatePluginResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;

        self.worker_service
            .activate_plugin(&worker_id, &plugin_installation_id, namespace)
            .await?;

        Ok(Json(ActivatePluginResponse {}))
    }

    /// Deactivate a plugin
    ///
    /// The plugin must be one of the installed plugins for the worker's current component version.
    #[oai(
        path = "/:component_id/workers/:worker_name/deactivate-plugin",
        method = "post",
        operation_id = "deactivate_plugin"
    )]
    async fn deactivate_plugin(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        #[oai(name = "plugin-installation-id")] plugin_installation_id: Query<PluginInstallationId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeactivatePluginResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!(
            "activate_plugin",
            worker_id = worker_id.to_string(),
            plugin_installation_id = plugin_installation_id.to_string()
        );

        let response = self
            .deactivate_plugin_internal(worker_id, plugin_installation_id.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn deactivate_plugin_internal(
        &self,
        worker_id: WorkerId,
        plugin_installation_id: PluginInstallationId,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeactivatePluginResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;

        self.worker_service
            .deactivate_plugin(&worker_id, &plugin_installation_id, namespace)
            .await?;

        Ok(Json(DeactivatePluginResponse {}))
    }

    /// Revert a worker
    ///
    /// Reverts a worker by undoing either the last few invocations or the last few recorded oplog entries.
    #[oai(
        path = "/:component_id/workers/:worker_name/revert",
        method = "post",
        operation_id = "revert_worker"
    )]
    async fn revert_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        target: Json<RevertWorkerTarget>,
        token: GolemSecurityScheme,
    ) -> Result<Json<RevertWorkerResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record =
            recorded_http_api_request!("revert_worker", worker_id = worker_id.to_string(),);

        let response = self
            .revert_worker_internal(worker_id, target.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn revert_worker_internal(
        &self,
        worker_id: WorkerId,
        target: RevertWorkerTarget,
        token: GolemSecurityScheme,
    ) -> Result<Json<RevertWorkerResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;

        self.worker_service
            .revert_worker(&worker_id, target, namespace)
            .await?;

        Ok(Json(RevertWorkerResponse {}))
    }

    /// Cancels a pending invocation if it has not started yet
    ///
    /// The invocation to be cancelled is identified by the idempotency key passed to the invoke API.
    #[oai(
        path = "/:component_id/workers/:worker_name/invocations/:idempotency_key",
        method = "delete",
        operation_id = "cancel_invocation"
    )]
    async fn cancel_invocation(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        idempotency_key: Path<IdempotencyKey>,
        token: GolemSecurityScheme,
    ) -> Result<Json<CancelInvocationResponse>> {
        let worker_id = validated_worker_id(component_id.0, worker_name.0)?;

        let record = recorded_http_api_request!(
            "cancel_invocation",
            worker_id = worker_id.to_string(),
            idempotency_key = idempotency_key.0.to_string(),
        );

        let response = self
            .cancel_invocation_internal(worker_id, idempotency_key.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn cancel_invocation_internal(
        &self,
        worker_id: WorkerId,
        idempotency_key: IdempotencyKey,
        token: GolemSecurityScheme,
    ) -> Result<Json<CancelInvocationResponse>> {
        let auth = CloudAuthCtx::new(token.secret());
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::UpdateWorker, &auth)
            .await?;

        let canceled = self
            .worker_service
            .cancel_invocation(&worker_id, &idempotency_key, namespace)
            .await?;

        Ok(Json(CancelInvocationResponse { canceled }))
    }

    /// Connect to a worker using a websocket and stream events
    #[oai(
        path = "/:component_id/workers/:worker_name/connect",
        method = "get",
        operation_id = "worker_connect"
    )]
    pub async fn worker_connect(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        websocket: WebSocket,
        token: WrappedGolemSecuritySchema,
    ) -> Result<BoxWebSocketUpgraded> {
        let (worker_id, worker_stream) = self
            .connect_to_worker(component_id.0, worker_name.0, token.0.secret())
            .await?;

        let upgraded: BoxWebSocketUpgraded = websocket.on_upgrade(Box::new(|socket_stream| {
            Box::pin(async move {
                let (sink, stream) = socket_stream.split();
                let _ = proxy_worker_connection(
                    worker_id,
                    worker_stream,
                    sink,
                    stream,
                    WORKER_CONNECT_PING_INTERVAL,
                    WORKER_CONNECT_PING_TIMEOUT,
                )
                .await;
            })
        }));

        Ok(upgraded)
    }

    async fn connect_to_worker(
        &self,
        component_id: ComponentId,
        worker_name: String,
        token: TokenSecret,
    ) -> Result<(WorkerId, ConnectWorkerStream)> {
        let worker_id = validated_worker_id(component_id, worker_name)?;

        let record =
            recorded_http_api_request!("connect_worker", worker_id = worker_id.to_string());

        let response = self
            .connect_to_worker_internal(worker_id.clone(), token)
            .instrument(record.span.clone())
            .await
            .map(|stream| (worker_id, stream));

        record.result(response)
    }

    async fn connect_to_worker_internal(
        &self,
        worker_id: WorkerId,
        token: TokenSecret,
    ) -> Result<ConnectWorkerStream> {
        let auth = CloudAuthCtx::new(token);
        let namespace = self
            .worker_auth_service
            .is_authorized_by_component(&worker_id.component_id, ProjectAction::ViewWorker, &auth)
            .await
            .map_err(|e| {
                WorkerError::Unauthorized(Json(ErrorBody {
                    error: format!("Unauthorized: {e}"),
                }))
            })?;

        let stream = self.worker_service.connect(&worker_id, namespace).await?;
        Ok(stream)
    }
}

// TODO: should be in a base library
fn validated_worker_id(
    component_id: ComponentId,
    worker_name: String,
) -> std::result::Result<WorkerId, WorkerError> {
    WorkerId::validate_worker_name(&worker_name)
        .map_err(|error| WorkerError::bad_request(format!("Invalid worker name: {error}")))?;
    Ok(WorkerId {
        component_id,
        worker_name,
    })
}

// TODO: should be in a base library
fn make_target_worker_id(
    component_id: ComponentId,
    worker_name: Option<String>,
) -> std::result::Result<TargetWorkerId, WorkerError> {
    if let Some(worker_name) = &worker_name {
        WorkerId::validate_worker_name(worker_name).map_err(|error| {
            WorkerError::BadRequest(Json(ErrorsBody {
                errors: vec![format!("Invalid worker name: {error}")],
            }))
        })?;
    }

    Ok(TargetWorkerId {
        component_id,
        worker_name,
    })
}

// TODO: should be in a base library
fn make_component_file_path(name: String) -> std::result::Result<ComponentFilePath, WorkerError> {
    ComponentFilePath::from_rel_str(&name).map_err(|error| {
        WorkerError::BadRequest(Json(ErrorsBody {
            errors: vec![format!("Invalid file name: {error}")],
        }))
    })
}
