use std::sync::Arc;

use cloud_common::auth::GolemSecurityScheme;
use golem_common::model::{CallingConvention, InvocationKey, TemplateId, WorkerFilter};
use golem_worker_service_base::service::template::TemplateServiceError;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use tap::TapFallible;
use tonic::Status;

use crate::api::ApiTags;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::template::{TemplateError, TemplateService};
use crate::service::worker::{WorkerError as WorkerServiceError, WorkerService};
use golem_service_base::model::*;

#[derive(ApiResponse)]
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
    /// Template / Worker / Promise not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    /// Worker already exists
    #[oai(status = 409)]
    AlreadyExists(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<GolemErrorBody>),
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

impl From<TemplateServiceError> for WorkerError {
    fn from(value: TemplateServiceError) -> Self {
        match value {
            TemplateServiceError::BadRequest(errors) => {
                WorkerError::BadRequest(Json(ErrorsBody { errors }))
            }
            TemplateServiceError::AlreadyExists(error) => {
                WorkerError::AlreadyExists(Json(ErrorBody { error }))
            }
            TemplateServiceError::Internal(error) => {
                WorkerError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown {
                        details: error.to_string(),
                    }),
                }))
            }
            TemplateServiceError::Unauthorized(error) => {
                WorkerError::Unauthorized(Json(ErrorBody { error }))
            }
            TemplateServiceError::Forbidden(error) => {
                WorkerError::LimitExceeded(Json(ErrorBody { error }))
            }
            TemplateServiceError::NotFound(error) => {
                WorkerError::NotFound(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<WorkerServiceError> for WorkerError {
    fn from(value: WorkerServiceError) -> Self {
        use golem_worker_service_base::service::worker::WorkerServiceError as BaseServiceError;

        match value {
            WorkerServiceError::Forbidden(error) => {
                WorkerError::LimitExceeded(Json(ErrorBody { error }))
            }
            WorkerServiceError::Unauthorized(error) => {
                WorkerError::Unauthorized(Json(ErrorBody { error }))
            }
            WorkerServiceError::ProjectNotFound(_) => WorkerError::NotFound(Json(ErrorBody {
                error: value.to_string(),
            })),
            WorkerServiceError::Base(error) => match error {
                BaseServiceError::VersionedTemplateIdNotFound(_)
                | BaseServiceError::TemplateNotFound(_)
                | BaseServiceError::AccountIdNotFound(_)
                | BaseServiceError::WorkerNotFound(_) => WorkerError::NotFound(Json(ErrorBody {
                    error: error.to_string(),
                })),
                BaseServiceError::TypeChecker(error) => WorkerError::bad_request(error),
                BaseServiceError::Template(error) => error.into(),
                BaseServiceError::Internal(error) => {
                    WorkerError::InternalError(Json(GolemErrorBody {
                        golem_error: GolemError::Unknown(GolemErrorUnknown {
                            details: error.to_string(),
                        }),
                    }))
                }
                BaseServiceError::Golem(golem_error) => {
                    WorkerError::InternalError(Json(GolemErrorBody { golem_error }))
                }
            },
        }
    }
}

impl From<AuthServiceError> for WorkerError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => {
                WorkerError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Unexpected(error) => {
                WorkerError::InternalError(Json(GolemErrorBody {
                    golem_error: GolemError::Unknown(GolemErrorUnknown { details: error }),
                }))
            }
        }
    }
}

impl From<TemplateError> for WorkerError {
    fn from(error: TemplateError) -> Self {
        match error {
            TemplateError::AlreadyExists(_) => WorkerError::AlreadyExists(Json(ErrorBody {
                error: error.to_string(),
            })),
            TemplateError::UnknownTemplateId(_)
            | TemplateError::UnknownVersionedTemplateId(_)
            | TemplateError::UnknownProjectId(_) => WorkerError::NotFound(Json(ErrorBody {
                error: error.to_string(),
            })),
            TemplateError::Unauthorized(error) => {
                WorkerError::Unauthorized(Json(ErrorBody { error }))
            }
            TemplateError::LimitExceeded(error) => {
                WorkerError::LimitExceeded(Json(ErrorBody { error }))
            }
            TemplateError::TemplateProcessing(_) => WorkerError::BadRequest(Json(ErrorsBody {
                errors: vec![error.to_string()],
            })),
            TemplateError::Internal(_) => WorkerError::InternalError(Json(GolemErrorBody {
                golem_error: GolemError::Unknown(GolemErrorUnknown {
                    details: error.to_string(),
                }),
            })),
        }
    }
}

pub struct WorkerApi {
    pub template_service: Arc<dyn TemplateService + Sync + Send>,
    pub worker_service: Arc<dyn WorkerService + Sync + Send>,
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/templates", tag = ApiTags::Worker)]
impl WorkerApi {
    /// Launch a new worker.
    ///
    /// Creates a new worker. The worker initially is in `Idle`` status, waiting to be invoked.
    ///
    /// The parameters in the request are the following:
    /// - `name` is the name of the created worker. This has to be unique, but only for a given template
    /// - `args` is a list of strings which appear as command line arguments for the worker
    /// - `env` is a list of key-value pairs (represented by arrays) which appear as environment variables for the worker
    #[oai(
        path = "/:template_id/workers",
        method = "post",
        operation_id = "launch_new_worker"
    )]
    async fn launch_new_worker(
        &self,
        template_id: Path<TemplateId>,
        request: Json<WorkerCreationRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<VersionedWorkerId>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        let template_id = template_id.0;
        let latest_template = self
            .template_service
            .get_latest_version(&template_id, &auth)
            .await
            .tap_err(|error| tracing::error!("Error getting latest template version: {:?}", error))?
            .ok_or(WorkerError::NotFound(Json(ErrorBody {
                error: format!("Template not found: {}", &template_id),
            })))?;

        let WorkerCreationRequest { name, args, env } = request.0;

        let worker_id = make_worker_id(template_id, name)?;

        let worker = self
            .worker_service
            .create(
                &worker_id,
                latest_template.versioned_template_id.version,
                args,
                env,
                &auth,
            )
            .await?;

        Ok(Json(worker))
    }

    /// Delete a worker
    ///
    /// Interrupts and deletes an existing worker.
    #[oai(
        path = "/:template_id/workers/:worker_name",
        method = "delete",
        operation_id = "delete_worker"
    )]
    async fn delete_worker(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeleteWorkerResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        self.worker_service.delete(&worker_id, &auth).await?;

        Ok(Json(DeleteWorkerResponse {}))
    }

    /// Get an invocation key
    ///
    /// Creates an invocation key for a given worker.
    /// An invocation key is passed to the below defined invoke APIs to guarantee that retrying those invocations only performs the operation on the worker once.
    #[oai(
        path = "/:template_id/workers/:worker_name/key",
        method = "post",
        operation_id = "get_invocation_key"
    )]
    async fn get_invocation_key(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvocationKey>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        let invocation_key = self
            .worker_service
            .get_invocation_key(&worker_id, &auth)
            .await?;

        Ok(Json(invocation_key))
    }

    /// Invoke a function and await it's resolution
    ///
    /// Supply the parameters in the request body as JSON.
    #[oai(
        path = "/:template_id/workers/:worker_name/invoke-and-await",
        method = "post",
        operation_id = "invoke_and_await_function"
    )]
    async fn invoke_and_await_function(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        /// must be created with the create invokation key endpoint
        #[oai(name = "invocation-key")]
        invocation_key: Query<String>,
        /// name of the exported function to be invoked
        function: Query<String>,
        /// One of `component`, `stdio`, `stdio-event-loop`. Defaults to `component`.
        #[oai(name = "calling-convention")]
        calling_convention: Query<Option<CallingConvention>>,
        params: Json<InvokeParameters>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvokeResult>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        let calling_convention = calling_convention.0.unwrap_or(CallingConvention::Component);

        let result = self
            .worker_service
            .invoke_and_await_function(
                &worker_id,
                function.0,
                &InvocationKey {
                    value: invocation_key.0,
                },
                params.0.params,
                &calling_convention,
                &auth,
            )
            .await?;

        Ok(Json(InvokeResult { result }))
    }

    /// Invoke a function
    ///
    /// A simpler version of the previously defined invoke and await endpoint just triggers the execution of a function and immediately returns. Custom calling convention and invocation key is not supported.
    /// To understand how to get the function name and how to encode the function parameters check Template interface
    #[oai(
        path = "/:template_id/workers/:worker_name/invoke",
        method = "post",
        operation_id = "invoke_function"
    )]
    async fn invoke_function(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        /// name of the exported function to be invoked
        function: Query<String>,
        params: Json<InvokeParameters>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvokeResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        self.worker_service
            .invoke_function(&worker_id, function.0, params.0.params, &auth)
            .await?;

        Ok(Json(InvokeResponse {}))
    }

    /// Complete a promise
    ///
    /// Completes a promise with a given custom array of bytes.
    /// The promise must be previously created from within the worker, and it's identifier (a combination of a worker identifier and an oplogIdx ) must be sent out to an external caller so it can use this endpoint to mark the promise completed.
    /// The data field is sent back to the worker and it has no predefined meaning.
    #[oai(
        path = "/:template_id/workers/:worker_name/complete",
        method = "post",
        operation_id = "complete_promise"
    )]
    async fn complete_promise(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        params: Json<CompleteParameters>,
        token: GolemSecurityScheme,
    ) -> Result<Json<bool>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;
        let CompleteParameters { oplog_idx, data } = params.0;

        let result = self
            .worker_service
            .complete_promise(&worker_id, oplog_idx, data, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Interrupt a worker
    ///
    /// Interrupts the execution of a worker.
    /// The worker's status will be Interrupted unless the recover-immediately parameter was used, in which case it remains as it was.
    /// An interrupted worker can be still used, and it is going to be automatically resumed the first time it is used.
    /// For example in case of a new invocation, the previously interrupted invocation is continued before the new one gets processed.
    #[oai(
        path = "/:template_id/workers/:worker_name/interrupt",
        method = "post",
        operation_id = "interrupt_worker"
    )]
    async fn interrupt_worker(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        /// if true will simulate a worker recovery. Defaults to false.
        #[oai(name = "recovery-immediately")]
        recover_immediately: Query<Option<bool>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InterruptResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        self.worker_service
            .interrupt(&worker_id, recover_immediately.0.unwrap_or(false), &auth)
            .await?;

        Ok(Json(InterruptResponse {}))
    }

    /// Get metadata of a worker
    ///
    /// Returns metadata about an existing worker:
    /// - `workerId` is a combination of the used template and the worker's user specified name
    /// - `accountId` the account the worker is created by
    /// - `args` is the provided command line arguments passed to the worker
    /// - `env` is the provided map of environment variables passed to the worker
    /// - `templateVersion` is the version of the template used by the worker
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
        path = "/:template_id/workers/:worker_name",
        method = "get",
        operation_id = "get_worker_metadata"
    )]
    async fn get_worker_metadata(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::WorkerMetadata>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;
        let result = self.worker_service.get_metadata(&worker_id, &auth).await?;

        Ok(Json(result))
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
    /// Returns metadata about an existing template workers:
    /// - `workers` list of workers metadata
    /// - `cursor` cursor for next request, if cursor is empty/null, there are no other values
    #[oai(
        path = "/:template_id/workers",
        method = "get",
        operation_id = "get_workers_metadata"
    )]
    async fn get_workers_metadata(
        &self,
        template_id: Path<TemplateId>,
        /// Filter for worker metadata in form of `property op value`. Can be used multiple times (AND condition is applied between them)
        filter: Query<Option<Vec<String>>>,
        /// Count of listed values, default: 50
        cursor: Query<Option<u64>>,
        /// Position where to start listing, if not provided, starts from the beginning. It is used to get the next page of results. To get next page, use the cursor returned in the response
        count: Query<Option<u64>>,
        /// Precision in relation to worker status, if true, calculate the most up-to-date status for each worker, default is false
        precise: Query<Option<bool>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::WorkersMetadataResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        let filter = match filter.0 {
            Some(filters) if !filters.is_empty() => Some(
                WorkerFilter::from(filters)
                    .map_err(|e| WorkerError::BadRequest(Json(ErrorsBody { errors: vec![e] })))?,
            ),
            _ => None,
        };

        let (cursor, workers) = self
            .worker_service
            .find_metadata(
                &template_id.0,
                filter,
                cursor.0.unwrap_or(0),
                count.0.unwrap_or(50),
                precise.0.unwrap_or(false),
                &auth,
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
    /// Returns metadata about an existing template workers:
    /// - `workers` list of workers metadata
    /// - `cursor` cursor for next request, if cursor is empty/null, there are no other values
    #[oai(
        path = "/:template_id/workers/find",
        method = "post",
        operation_id = "find_workers_metadata"
    )]
    async fn find_workers_metadata(
        &self,
        template_id: Path<TemplateId>,
        params: Json<WorkersMetadataRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<crate::model::WorkersMetadataResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        let (cursor, workers) = self
            .worker_service
            .find_metadata(
                &template_id.0,
                params.filter.clone(),
                params.cursor.unwrap_or(0),
                params.count.unwrap_or(50),
                params.precise.unwrap_or(false),
                &auth,
            )
            .await?;

        Ok(Json(crate::model::WorkersMetadataResponse {
            workers,
            cursor,
        }))
    }

    /// Resume a worker
    #[oai(
        path = "/:template_id/workers/:worker_name/resume",
        method = "post",
        operation_id = "resume_worker"
    )]
    async fn resume_worker(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ResumeResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        self.worker_service.resume(&worker_id, &auth).await?;

        Ok(Json(ResumeResponse {}))
    }
}

fn make_worker_id(
    template_id: TemplateId,
    worker_name: String,
) -> std::result::Result<WorkerId, WorkerError> {
    WorkerId::new(template_id, worker_name)
        .map_err(|error| WorkerError::bad_request(format!("Invalid worker name: {error}")))
}
