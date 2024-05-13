use crate::worker_binding::{RequestDetails, ResponseMapping};
use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerResponse};

pub trait ToResponse<A> {
    fn to_response(&self, response_mapping: &Option<ResponseMapping>, request_details: &RequestDetails) -> A;
}

impl ToResponse<poem::Response> for WorkerResponse {
    fn to_response(&self, response_mapping: &Option<ResponseMapping>, request_details: &RequestDetails) -> poem::Response {
        let refined_worker_response = RefinedWorkerResponse::from_worker_response(self);

        match refined_worker_response {
            Ok(refined) =>
                refined.to_http_response(response_mapping, request_details),
            Err(e) => {
                poem::Response::builder()
                    .status(poem::http::StatusCode::INTERNAL_SERVER_ERROR)
                    .body(poem::Body::from_string(
                        format!("API request error {}", e).to_string(),
                    ))
            }
        }


    }
}
