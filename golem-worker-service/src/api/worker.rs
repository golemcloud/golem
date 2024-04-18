use golem_common::model::{CallingConvention, InvocationKey, TemplateId, WorkerFilter};
use golem_service_base::api_tags::ApiTags;
use golem_worker_service_base::auth::EmptyAuthCtx;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use tap::TapFallible;

use golem_service_base::model::*;
use golem_worker_service_base::api::WorkerApiBaseError;

use crate::empty_worker_metadata;
use crate::service::{template::TemplateService, worker::WorkerService};

pub struct WorkerApi {
    pub template_service: TemplateService,
    pub worker_service: WorkerService,
}

type Result<T> = std::result::Result<T, WorkerApiBaseError>;

#[OpenApi(prefix_path = "/v2/templates", tag = ApiTags::Worker)]
impl WorkerApi {
    #[oai(
        path = "/:template_id/workers",
        method = "post",
        operation_id = "launch_new_worker"
    )]
    async fn launch_new_worker(
        &self,
        template_id: Path<TemplateId>,
        request: Json<WorkerCreationRequest>,
    ) -> Result<Json<WorkerCreationResponse>> {
        let template_id = template_id.0;
        let latest_template = self
            .template_service
            .get_latest(&template_id, &EmptyAuthCtx {})
            .await
            .tap_err(|error| tracing::error!("Error getting latest template: {:?}", error))
            .map_err(|error| {
                WorkerApiBaseError::NotFound(Json(ErrorBody {
                    error: format!(
                        "Couldn't retrieve the template not found: {}. error: {}",
                        &template_id, error
                    ),
                }))
            })?;

        let WorkerCreationRequest { name, args, env } = request.0;

        let worker_id = make_worker_id(template_id, name)?;
        let worker_id = self
            .worker_service
            .create(
                &worker_id,
                latest_template.versioned_template_id.version,
                args,
                env,
                empty_worker_metadata(),
                &EmptyAuthCtx {},
            )
            .await?;

        Ok(Json(WorkerCreationResponse {
            worker_id,
            component_version: latest_template.versioned_template_id.version,
        }))
    }

    #[oai(
        path = "/:template_id/workers/:worker_name",
        method = "delete",
        operation_id = "delete_worker"
    )]
    async fn delete_worker(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
    ) -> Result<Json<DeleteWorkerResponse>> {
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        self.worker_service
            .delete(&worker_id, &EmptyAuthCtx {})
            .await?;

        Ok(Json(DeleteWorkerResponse {}))
    }

    #[oai(
        path = "/:template_id/workers/:worker_name/key",
        method = "post",
        operation_id = "get_invocation_key"
    )]
    async fn get_invocation_key(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
    ) -> Result<Json<InvocationKey>> {
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        let invocation_key = self
            .worker_service
            .get_invocation_key(&worker_id, &EmptyAuthCtx {})
            .await?;

        Ok(Json(invocation_key))
    }

    #[oai(
        path = "/:template_id/workers/:worker_name/invoke-and-await",
        method = "post",
        operation_id = "invoke_and_await_function"
    )]
    async fn invoke_and_await_function(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        #[oai(name = "invocation-key")] invocation_key: Query<String>,
        function: Query<String>,
        #[oai(name = "calling-convention")] calling_convention: Query<Option<CallingConvention>>,
        params: Json<InvokeParameters>,
    ) -> Result<Json<InvokeResult>> {
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
                empty_worker_metadata(),
                &EmptyAuthCtx {},
            )
            .await?;

        Ok(Json(InvokeResult { result }))
    }

    #[oai(
        path = "/:template_id/workers/:worker_name/invoke",
        method = "post",
        operation_id = "invoke_function"
    )]
    async fn invoke_function(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        function: Query<String>,
        params: Json<InvokeParameters>,
    ) -> Result<Json<InvokeResponse>> {
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        self.worker_service
            .invoke_function(
                &worker_id,
                function.0,
                params.0.params,
                empty_worker_metadata(),
                &EmptyAuthCtx {},
            )
            .await?;

        Ok(Json(InvokeResponse {}))
    }

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
    ) -> Result<Json<bool>> {
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;
        let CompleteParameters { oplog_idx, data } = params.0;

        let result = self
            .worker_service
            .complete_promise(&worker_id, oplog_idx, data, &EmptyAuthCtx {})
            .await?;

        Ok(Json(result))
    }

    #[oai(
        path = "/:template_id/workers/:worker_name/interrupt",
        method = "post",
        operation_id = "interrupt_worker"
    )]
    async fn interrupt_worker(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
        #[oai(name = "recovery-immediately")] recover_immediately: Query<Option<bool>>,
    ) -> Result<Json<InterruptResponse>> {
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        self.worker_service
            .interrupt(
                &worker_id,
                recover_immediately.0.unwrap_or(false),
                &EmptyAuthCtx {},
            )
            .await?;

        Ok(Json(InterruptResponse {}))
    }

    #[oai(
        path = "/:template_id/workers/:worker_name",
        method = "get",
        operation_id = "get_worker_metadata"
    )]
    async fn get_worker_metadata(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
    ) -> Result<Json<WorkerMetadata>> {
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;
        let result = self
            .worker_service
            .get_metadata(&worker_id, &EmptyAuthCtx {})
            .await?;

        Ok(Json(result))
    }

    #[oai(
        path = "/:template_id/workers",
        method = "get",
        operation_id = "get_workers_metadata"
    )]
    async fn get_workers_metadata(
        &self,
        template_id: Path<TemplateId>,
        filter: Query<Option<Vec<String>>>,
        cursor: Query<Option<u64>>,
        count: Query<Option<u64>>,
        precise: Query<Option<bool>>,
    ) -> Result<Json<WorkersMetadataResponse>> {
        let filter = match filter.0 {
            Some(filters) if !filters.is_empty() => {
                Some(WorkerFilter::from(filters).map_err(|e| {
                    WorkerApiBaseError::BadRequest(Json(ErrorsBody { errors: vec![e] }))
                })?)
            }
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
                &EmptyAuthCtx {},
            )
            .await?;

        Ok(Json(WorkersMetadataResponse { workers, cursor }))
    }

    #[oai(
        path = "/:template_id/workers/find",
        method = "post",
        operation_id = "find_workers_metadata"
    )]
    async fn find_workers_metadata(
        &self,
        template_id: Path<TemplateId>,
        params: Json<WorkersMetadataRequest>,
    ) -> Result<Json<WorkersMetadataResponse>> {
        let (cursor, workers) = self
            .worker_service
            .find_metadata(
                &template_id.0,
                params.filter.clone(),
                params.cursor.unwrap_or(0),
                params.count.unwrap_or(50),
                params.precise.unwrap_or(false),
                &EmptyAuthCtx {},
            )
            .await?;

        Ok(Json(WorkersMetadataResponse { workers, cursor }))
    }

    #[oai(
        path = "/:template_id/workers/:worker_name/resume",
        method = "post",
        operation_id = "resume_worker"
    )]
    async fn resume_worker(
        &self,
        template_id: Path<TemplateId>,
        worker_name: Path<String>,
    ) -> Result<Json<ResumeResponse>> {
        let worker_id = make_worker_id(template_id.0, worker_name.0)?;

        self.worker_service
            .resume(&worker_id, &EmptyAuthCtx {})
            .await?;

        Ok(Json(ResumeResponse {}))
    }
}

fn make_worker_id(
    template_id: TemplateId,
    worker_name: String,
) -> std::result::Result<WorkerId, WorkerApiBaseError> {
    WorkerId::new(template_id, worker_name).map_err(|error| {
        WorkerApiBaseError::BadRequest(Json(ErrorsBody {
            errors: vec![format!("Invalid worker name: {error}")],
        }))
    })
}
