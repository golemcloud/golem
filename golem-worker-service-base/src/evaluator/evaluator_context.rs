use golem_wasm_rpc::TypeAnnotatedValue;
use crate::evaluator::EvaluationResult;
use crate::merge::Merge;
use crate::worker_binding::{RequestDetails};
use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerRequest, WorkerResponse};


// Evaluator of an expression doesn't necessarily need a context all the time, and can be empty.
// or contain worker details, request details, worker_response or all of them.

#[derive(Clone)]
pub struct EvaluationContext {
    pub worker_request: Option<WorkerRequest>,
    pub worker_response: Option<RefinedWorkerResponse>,
    pub variables: Option<TypeAnnotatedValue>,
    pub request_data: Option<RequestDetails>
}

impl EvaluationContext {

    pub fn merge(&mut self, variables: &TypeAnnotatedValue) {
        match self.variables {
            Some(ref mut existing) => {
                existing.merge(variables);
            },
            None => {
                self.variables = Some(variables.clone());
            }
        }
    }

    pub fn merge_worker_data(&self) -> Option<EvaluationResult> {
        match (&self.worker_response, &self.worker_request) {
            (Some(res), Some(req)) => {
                let mut typed_worker_data = req.clone().to_type_annotated_value();

                if let Some(typed_res) = res.to_type_annotated_value() {
                    typed_worker_data.merge(&typed_res);
                }

               Some(EvaluationResult::Value(typed_worker_data))
            },

            (None, Some(req)) => Some(req.clone().to_type_annotated_value().into()),
            (Some(res), None) => match res {
                RefinedWorkerResponse::Unit => Some(EvaluationResult::Unit),
                RefinedWorkerResponse::SingleResult(value) => Some(value.clone().into()),
                RefinedWorkerResponse::MultipleResults(value) => Some(value.clone().into())
            }
            (None, None) => None
        }
    }


    pub fn from_worker_data(worker_metadata: &WorkerRequest) -> Self {
       EvaluationContext {
              worker_request: Some(worker_metadata.clone()),
              worker_response: None,
              variables: None,
              request_data: None
       }
    }

    pub fn from_worker_response(worker_response: &RefinedWorkerResponse) -> Self {
        EvaluationContext {
            worker_request: None,
            worker_response: Some(worker_response.clone()),
            variables: None,
            request_data: None
        }
    }

    pub fn from_request_data(request: &RequestDetails) -> Self {
        EvaluationContext {
            worker_request: None,
            worker_response: None,
            variables: None,
            request_data: Some(request.clone())
        }
    }

    pub fn from_variables(variables: &TypeAnnotatedValue) -> Self {
        EvaluationContext {
            worker_request: None,
            worker_response: None,
            variables: Some(variables.clone()),
            request_data: None
        }
    }

    pub fn from(worker_request: &WorkerRequest, worker_response: &RefinedWorkerResponse, request: &RequestDetails) -> Self {
        EvaluationContext {
            worker_request: Some(worker_request.clone()),
            worker_response: Some(worker_response.clone()),
            variables: None,
            request_data: Some(request.clone())
        }
    }

}