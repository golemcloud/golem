use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::CallingConvention;
use golem_service_base::model::WorkerId;
use golem_wasm_rpc::TypeAnnotatedValue;
use golem_worker_service_base::http_api_definition::ResponseMapping;
use golem_worker_service_base::auth::EmptyAuthCtx;
use golem_worker_service_base::service::worker::WorkerService;
use golem_worker_service_base::worker_request::WorkerRequest;
use golem_worker_service_base::worker_request_to_response::WorkerRequestToResponse;
use golem_worker_service_base::worker_response::WorkerResponse;
use http::StatusCode;
use poem::Body;
use tracing::info;

use crate::empty_worker_metadata;

pub struct WorkerRequestToHttpResponse {
    pub worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Sync + Send>,
}

impl WorkerRequestToHttpResponse {
    pub fn new(worker_service: Arc<dyn WorkerService<EmptyAuthCtx> + Sync + Send>) -> Self {
        Self { worker_service }
    }
}

#[async_trait]
impl WorkerRequestToResponse<ResponseMapping, poem::Response> for WorkerRequestToHttpResponse {
    async fn execute(
        &self,
        worker_request_params: WorkerRequest,
        response_mapping: &Option<ResponseMapping>,
        input_request: &TypeAnnotatedValue,
    ) -> poem::Response {
        match execute(self, worker_request_params.clone()).await {
            Ok(worker_response) => {
                worker_response.to_http_response(response_mapping, input_request)
            }
            Err(e) => poem::Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from_string(format!(
                    "Error when executing resolved worker request. Error: {}",
                    e
                ))),
        }
    }
}

async fn execute(
    default_executor: &WorkerRequestToHttpResponse,
    worker_request_params: WorkerRequest,
) -> Result<WorkerResponse, Box<dyn Error>> {
    let worker_name = worker_request_params.worker_id;

    let template_id = worker_request_params.template;

    let worker_id = WorkerId::new(template_id.clone(), worker_name.clone())?;

    info!(
        "Executing request for template: {}, worker: {}, function: {}",
        template_id,
        worker_name.clone(),
        worker_request_params.function
    );

    let invocation_key = default_executor
        .worker_service
        .get_invocation_key(&worker_id, &EmptyAuthCtx {})
        .await
        .map_err(|e| e.to_string())?;

    let invoke_parameters = worker_request_params.function_params;

    info!(
            "Executing request for template: {}, worker: {}, invocation key: {}, invocation params: {:?}",
            template_id, worker_name.clone(), invocation_key, invoke_parameters
        );

    let invoke_result = default_executor
        .worker_service
        .invoke_and_await_function_typed_value(
            &worker_id,
            worker_request_params.function,
            &invocation_key,
            invoke_parameters,
            &CallingConvention::Component,
            empty_worker_metadata(),
            &EmptyAuthCtx {},
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(WorkerResponse {
        result: invoke_result,
    })
}
