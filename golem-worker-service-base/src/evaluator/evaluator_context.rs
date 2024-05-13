use golem_wasm_rpc::TypeAnnotatedValue;
use crate::evaluator::EvaluationResult;
use crate::merge::Merge;
use crate::worker_binding::{RequestDetails};
use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerRequest, WorkerResponse};
use crate::evaluator::Getter;
use crate::evaluator::getter::GetError;
use crate::evaluator::path::Path;


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
    pub fn empty() -> Self {
        EvaluationContext {
            worker_request: None,
            worker_response: None,
            variables: None,
            request_data: None
        }
    }


    pub fn merge_variables(&mut self, variables: &TypeAnnotatedValue) {
        match self.variables {
            Some(ref mut existing) => {
                existing.merge(variables);
            },
            None => {
                self.variables = Some(variables.clone());
            }
        }
    }

    pub fn get_variable_value(&self, variable_name: &str) -> Result<TypeAnnotatedValue, GetError> {
        match &self.variables {
            Some(variables) => {
                variables.get(&Path::from_key(variable_name))
            },
            None => Err(GetError::KeyNotFound(variable_name.to_string()))
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