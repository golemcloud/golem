use golem_common::model::{
    CallingConvention, ComponentId, IdempotencyKey, ScanCursor, WorkerFilter,
};
use golem_service_base::api_tags::ApiTags;
use golem_worker_service_base::auth::EmptyAuthCtx;
use poem_openapi::param::{Header, Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::str::FromStr;
use tap::TapFallible;

use golem_service_base::model::*;
use golem_worker_service_base::api::WorkerApiBaseError;

use crate::empty_worker_metadata;
use crate::service::{component::ComponentService, worker::WorkerService};

pub struct WorkerApi {
    pub component_service: ComponentService,
    pub worker_service: WorkerService,
}

type Result<T> = std::result::Result<T, WorkerApiBaseError>;

#[OpenApi(prefix_path = "/v2/components", tag = ApiTags::Worker)]
impl WorkerApi {
    #[oai(
        path = "/:component_id/workers",
        method = "post",
        operation_id = "launch_new_worker"
    )]
    async fn launch_new_worker(
        &self,
        component_id: Path<ComponentId>,
        request: Json<WorkerCreationRequest>,
    ) -> Result<Json<WorkerCreationResponse>> {
        let component_id = component_id.0;
        let latest_component = self
            .component_service
            .get_latest(&component_id, &EmptyAuthCtx::default())
            .await
            .tap_err(|error| tracing::error!("Error getting latest component: {:?}", error))
            .map_err(|error| {
                WorkerApiBaseError::NotFound(Json(ErrorBody {
                    error: format!(
                        "Couldn't retrieve the component: {}. error: {}",
                        &component_id, error
                    ),
                }))
            })?;

        let WorkerCreationRequest { name, args, env } = request.0;

        let worker_id = make_worker_id(component_id, name)?;
        let worker_id = self
            .worker_service
            .create(
                &worker_id,
                latest_component.versioned_component_id.version,
                args,
                env,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(WorkerCreationResponse {
            worker_id,
            component_version: latest_component.versioned_component_id.version,
        }))
    }

    #[oai(
        path = "/:component_id/workers/:worker_name",
        method = "delete",
        operation_id = "delete_worker"
    )]
    async fn delete_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
    ) -> Result<Json<DeleteWorkerResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        self.worker_service
            .delete(
                &worker_id,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(DeleteWorkerResponse {}))
    }

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
        #[oai(name = "calling-convention")] calling_convention: Query<Option<CallingConvention>>,
        params: Json<InvokeParameters>,
    ) -> Result<Json<InvokeResult>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        let calling_convention = calling_convention.0.unwrap_or(CallingConvention::Component);

        let result = self
            .worker_service
            .invoke_and_await_function(
                &worker_id,
                idempotency_key.0,
                function.0,
                params.0.params,
                &calling_convention,
                None,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(InvokeResult { result }))
    }

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
        function: Query<String>,
        params: Json<InvokeParameters>,
    ) -> Result<Json<InvokeResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        self.worker_service
            .invoke_function(
                &worker_id,
                idempotency_key.0,
                function.0,
                params.0.params,
                None,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(InvokeResponse {}))
    }

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
    ) -> Result<Json<bool>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;
        let CompleteParameters { oplog_idx, data } = params.0;

        let result = self
            .worker_service
            .complete_promise(
                &worker_id,
                oplog_idx,
                data,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(result))
    }

    #[oai(
        path = "/:component_id/workers/:worker_name/interrupt",
        method = "post",
        operation_id = "interrupt_worker"
    )]
    async fn interrupt_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
        #[oai(name = "recovery-immediately")] recover_immediately: Query<Option<bool>>,
    ) -> Result<Json<InterruptResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        self.worker_service
            .interrupt(
                &worker_id,
                recover_immediately.0.unwrap_or(false),
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(InterruptResponse {}))
    }

    #[oai(
        path = "/:component_id/workers/:worker_name",
        method = "get",
        operation_id = "get_worker_metadata"
    )]
    async fn get_worker_metadata(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
    ) -> Result<Json<WorkerMetadata>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;
        let result = self
            .worker_service
            .get_metadata(
                &worker_id,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(result))
    }

    #[oai(
        path = "/:component_id/workers",
        method = "get",
        operation_id = "get_workers_metadata"
    )]
    async fn get_workers_metadata(
        &self,
        component_id: Path<ComponentId>,
        filter: Query<Option<Vec<String>>>,
        cursor: Query<Option<String>>,
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

        let cursor = match cursor.0 {
            Some(cursor) => Some(ScanCursor::from_str(&cursor).map_err(|e| {
                WorkerApiBaseError::BadRequest(Json(ErrorsBody { errors: vec![e] }))
            })?),
            None => None,
        };

        let (cursor, workers) = self
            .worker_service
            .find_metadata(
                &component_id.0,
                filter,
                cursor.unwrap_or_default(),
                count.0.unwrap_or(50),
                precise.0.unwrap_or(false),
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(WorkersMetadataResponse { workers, cursor }))
    }

    #[oai(
        path = "/:component_id/workers/find",
        method = "post",
        operation_id = "find_workers_metadata"
    )]
    async fn find_workers_metadata(
        &self,
        component_id: Path<ComponentId>,
        params: Json<WorkersMetadataRequest>,
    ) -> Result<Json<WorkersMetadataResponse>> {
        let (cursor, workers) = self
            .worker_service
            .find_metadata(
                &component_id.0,
                params.filter.clone(),
                params.cursor.clone().unwrap_or_default(),
                params.count.unwrap_or(50),
                params.precise.unwrap_or(false),
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(WorkersMetadataResponse { workers, cursor }))
    }

    #[oai(
        path = "/:component_id/workers/:worker_name/resume",
        method = "post",
        operation_id = "resume_worker"
    )]
    async fn resume_worker(
        &self,
        component_id: Path<ComponentId>,
        worker_name: Path<String>,
    ) -> Result<Json<ResumeResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        self.worker_service
            .resume(
                &worker_id,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(ResumeResponse {}))
    }

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
    ) -> Result<Json<UpdateWorkerResponse>> {
        let worker_id = make_worker_id(component_id.0, worker_name.0)?;

        self.worker_service
            .update(
                &worker_id,
                params.mode.clone().into(),
                params.target_version,
                empty_worker_metadata(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(Json(UpdateWorkerResponse {}))
    }
}

fn make_worker_id(
    component_id: ComponentId,
    worker_name: String,
) -> std::result::Result<WorkerId, WorkerApiBaseError> {
    WorkerId::new(component_id, worker_name).map_err(|error| {
        WorkerApiBaseError::BadRequest(Json(ErrorsBody {
            errors: vec![format!("Invalid worker name: {error}")],
        }))
    })
}
