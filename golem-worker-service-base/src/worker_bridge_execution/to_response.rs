use crate::worker_binding::{RequestDetails, ResponseMapping};
use crate::worker_bridge_execution::{
    RefinedWorkerResponse, WorkerRequest, WorkerRequestExecutorError, WorkerResponse,
};
use http::StatusCode;
use poem::Body;

pub trait ToResponse<A> {
    fn to_response(
        &self,
        worker_request: &WorkerRequest,
        response_mapping: &Option<ResponseMapping>,
        request_details: &RequestDetails,
    ) -> A;
}

impl ToResponse<poem::Response> for WorkerResponse {
    fn to_response(
        &self,
        worker_request: &WorkerRequest,
        response_mapping: &Option<ResponseMapping>,
        request_details: &RequestDetails,
    ) -> poem::Response {
        let refined_worker_response = RefinedWorkerResponse::from_worker_response(self);

        match refined_worker_response {
            Ok(refined) => {
                refined.to_http_response(response_mapping, request_details, worker_request)
            }
            Err(e) => poem::Response::builder()
                .status(poem::http::StatusCode::INTERNAL_SERVER_ERROR)
                .body(poem::Body::from_string(
                    format!("API request error {}", e).to_string(),
                )),
        }
    }
}

impl ToResponse<poem::Response> for WorkerRequestExecutorError {
    fn to_response(
        &self,
        _worker_request: &WorkerRequest,
        _response_mapping: &Option<ResponseMapping>,
        _request_details: &RequestDetails,
    ) -> poem::Response {
        poem::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from_string(
                format!("API request error {}", self.to_string()).to_string(),
            ))
    }
}
