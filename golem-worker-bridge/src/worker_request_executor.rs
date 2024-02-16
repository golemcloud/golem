use std::error::Error;

use async_trait::async_trait;
use serde_json::json;
use tracing::info;
use golem_common::model::CallingConvention;
use golem_service_base::model::{Id, WorkerId};

use crate::service::worker::{WorkerService, WorkerServiceDefault};
use crate::worker_request::GolemWorkerRequest;

#[async_trait]
pub trait WorkerRequestExecutor {
    async fn execute(&self, request: GolemWorkerRequest) -> Result<WorkerResponse, Box<dyn Error>>; // return type???
}

pub struct WorkerResponse {
    pub result: serde_json::Value,
}

pub struct WorkerRequestExecutorDefault {
    pub worker_service: WorkerServiceDefault,
}

#[async_trait]
impl WorkerRequestExecutor for WorkerRequestExecutorDefault {
    async fn execute(
        &self,
        worker_request_params: GolemWorkerRequest,
    ) -> Result<WorkerResponse, Box<dyn Error>> {
        let worker_name = worker_request_params.worker_id;

        let template_id = worker_request_params.template;

        let worker_id = WorkerId {
            template_id: template_id,
            worker_name: Id(worker_name.clone())
        };

        info!(
            "Executing request for template: {}, worker: {}, function: {}",
            template_id, worker_name, worker_request_params.function
        );

        let invocation_key = self
            .worker_service
            .get_invocation_key(&worker_id)
            .await
            .map_err(|e| e.to_string())?;

        let invoke_parameters = worker_request_params.function_params;

        info!(
            "Executing request for template: {}, worker: {}, invocation key: {}, invocation params: {:?}",
            template_id, worker_name, invocation_key, invoke_parameters
        );

        let invoke_result = self
            .worker_service
            .invoke_and_await_function(
                &worker_id,
                worker_request_params.function,
                &invocation_key,
                invoke_parameters,
                &CallingConvention::Component,
            )
            .await
            .map_err(|e| e.to_string())?;

        Ok(WorkerResponse {
            result: invoke_result,
        })
    }
}

pub struct NoOpWorkerRequestExecutor {}

#[async_trait]
impl WorkerRequestExecutor for NoOpWorkerRequestExecutor {
    async fn execute(
        &self,
        worker_request_params: GolemWorkerRequest,
    ) -> Result<WorkerResponse, Box<dyn Error>> {
        let worker_name = worker_request_params.worker_id;
        let template_id = worker_request_params.template;

        let worker_id = WorkerId {
            template_id: template_id,
            worker_name: Id(worker_name.clone())
        };

        info!(
            "Executing request for template: {}, worker: {}, function: {}",
            template_id, worker_name, worker_request_params.function
        );

        let sample_json_data = json!(
            [{
              "description" : "This is a sample in-memory response",
              "worker" : worker_name.0,
              "name": "John Doe",
              "age": 30,
              "email": "johndoe@example.com",
              "isStudent": false,
              "address": {
                "street": "123 Main Street",
                "city": "Anytown",
                "state": "CA",
                "postalCode": "12345"
              },
              "hobbies": ["reading", "hiking", "gaming"],
              "scores": [95, 88, 76, 92],
              "input" : worker_request_params.function_params.to_string()
            }]
        );

        Ok(WorkerResponse {
            result: sample_json_data,
        })
    }
}
