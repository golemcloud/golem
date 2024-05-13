use crate::worker_binding::{RequestDetails, WorkerDetails};
use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerRequest, WorkerResponse};


// Evaluator of an expression doesn't necessarily need a context all the time, and can be empty.
// or contain worker details, request details, worker_response or all of them.
pub enum EvaluatorInputContext {
    WorkerRequest(WorkerRequest),
    WorkerResponse(RefinedWorkerResponse),
    RequestData(RequestDetails),
    All {
        worker_request: WorkerRequest,
        worker_response: RefinedWorkerResponse,
        request: RequestDetails
    },
    Empty,
}

impl EvaluatorInputContext{
    pub fn from_worker_data(worker_metadata: WorkerRequest) -> Self {
        EvaluatorInputContext::WorkerRequest(worker_metadata)
    }

    pub fn from_worker_response(worker_response: RefinedWorkerResponse) -> Self {
        EvaluatorInputContext::WorkerResponse(worker_response)
    }

    pub fn from_request_data(request: &RequestDetails) -> Self {
        EvaluatorInputContext::RequestData(request.clone())
    }

    pub fn from_all(worker_request: WorkerRequest, worker_response: RefinedWorkerResponse, request: RequestDetails) -> Self {
        EvaluatorInputContext::All {
            worker_request: worker_request,
            worker_response: worker_response,
            request: request
        }
    }

}