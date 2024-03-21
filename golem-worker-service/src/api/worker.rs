use std::str::FromStr;
use std::sync::Arc;

use golem_common::model::{CallingConvention, InvocationKey, TemplateId};
use golem_service_base::api_tags::ApiTags;
use golem_worker_service_base::auth::EmptyAuthCtx;
use golem_worker_service_base::service::template::TemplateService;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use tap::TapFallible;

use golem_service_base::model::*;
use golem_worker_service_base::api::error::WorkerApiBaseError;

use crate::service::worker::WorkerService;

pub struct WorkerApi {
    pub template_service: Arc<dyn TemplateService + Sync + Send>,
    pub worker_service: WorkerService,
}

type Result<T> = std::result::Result<T, WorkerApiBaseError>;

#[OpenApi(prefix_path = "/v2/templates", tag = ApiTags::Worker)]
impl WorkerApi {
    #[oai(
        path = "/workers/:worker_id",
        method = "get",
        operation_id = "get_worker_by_id"
    )]
    async fn get_worker_by_id(&self, worker_id: Path<String>) -> Result<Json<VersionedWorkerId>> {
        let worker_id: WorkerId = golem_common::model::WorkerId::from_str(&worker_id.0)?.into();
        let (worker, _) = self
            .worker_service
            .get_by_id(&worker_id, &EmptyAuthCtx {})
            .await?;

        Ok(Json(worker))
    }

    #[oai(
        path = "/:template_id/workers",
        method = "post",
        operation_id = "launch_new_worker"
    )]
    async fn launch_new_worker(
        &self,
        template_id: Path<TemplateId>,
        request: Json<WorkerCreationRequest>,
    ) -> Result<Json<VersionedWorkerId>> {
        let template_id = template_id.0;
        let latest_template = self
            .template_service
            .get_latest(&template_id)
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
        let (worker, _) = self
            .worker_service
            .create(
                &worker_id,
                latest_template.versioned_template_id.version,
                args,
                env,
                &EmptyAuthCtx {},
            )
            .await?;

        Ok(Json(worker))
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

        let (invocation_key, _) = self
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

        let (result, _) = self
            .worker_service
            .invoke_and_await_function(
                &worker_id,
                function.0,
                &InvocationKey {
                    value: invocation_key.0,
                },
                params.0.params,
                &calling_convention,
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
            .invoke_function(&worker_id, function.0, params.0.params, &EmptyAuthCtx {})
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

        let (result, _) = self
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
        let (result, _) = self
            .worker_service
            .get_metadata(&worker_id, &EmptyAuthCtx {})
            .await?;

        Ok(Json(result))
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
