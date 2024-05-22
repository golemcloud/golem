use golem_wasm_ast::analysis::{AnalysedFunction};
use crate::evaluator::getter::GetError;
use crate::evaluator::path::Path;
use crate::evaluator::Getter;
use crate::merge::Merge;
use crate::worker_binding::RequestDetails;
use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerRequest};
use golem_wasm_rpc::TypeAnnotatedValue;

#[derive(Clone)]
pub struct EvaluationContext {
    pub variables: Option<TypeAnnotatedValue>,
    pub analysed_functions: Vec<AnalysedFunction>
}

impl EvaluationContext {
    pub fn empty() -> Self {
        EvaluationContext {
            variables: None,
            analysed_functions: vec![]
        }
    }

    pub fn merge_variables(&mut self, variables: &TypeAnnotatedValue) {
        match self.variables {
            Some(ref mut existing) => {
                existing.merge(variables);
            }
            None => {
                self.variables = Some(variables.clone());
            }
        }
    }

    pub fn get_variable_value(&self, variable_name: &str) -> Result<TypeAnnotatedValue, GetError> {
        match &self.variables {
            Some(variables) => variables.get(&Path::from_key(variable_name)),
            None => Err(GetError::KeyNotFound(variable_name.to_string())),
        }
    }

    pub fn from_request_data(request: &RequestDetails) -> Self {
        let variables = internal::request_type_annotated_value(request);

        EvaluationContext {
            variables: Some(variables),
            analysed_functions: vec![]
        }
    }

    #[allow(unused)]
    pub fn from_refined_worker_response(
        worker_response: &RefinedWorkerResponse,
    ) -> Self {
        let type_annoated_value = worker_response.to_type_annotated_value();


        if let Some(typed_res) = type_annoated_value {
            let response_data = internal::create_record("response", typed_res);
            let worker_data = internal::create_record("worker", response_data);

            EvaluationContext {
                variables: Some(worker_data),
                analysed_functions: vec![]
            }
        } else {
            EvaluationContext::empty()
        }
    }

    pub fn from(
        worker_request: &WorkerRequest,
        worker_response: &RefinedWorkerResponse,
        request: &RequestDetails,
    ) -> Self {
        let mut worker_request_data =
            internal::worker_request_type_annotated_value(worker_request);

        let worker_response_data =
            internal::worker_response_type_annotated_value(worker_response);

        if let Some(worker_response) = worker_response_data {
            let worker_data = worker_request_data.merge(&worker_response);
            let request_data = internal::request_type_annotated_value(request);

            let variables = worker_data.merge(&request_data).clone();

            EvaluationContext {
                variables: Some(variables),
                analysed_functions: vec![]
            }
        } else {
            EvaluationContext::from_request_data(request)
        }
    }
}

mod internal {
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerRequest};
    
    use crate::worker_binding::RequestDetails;

    pub(crate) fn request_type_annotated_value(request_details: &RequestDetails) -> TypeAnnotatedValue {
        let type_annoated_value = request_details.to_type_annotated_value();
        create_record("request", type_annoated_value)
    }

    pub(crate) fn worker_request_type_annotated_value(worker_request: &WorkerRequest) -> TypeAnnotatedValue {
        let typed_value = worker_request.clone().to_type_annotated_value();
        create_record("worker", typed_value)
    }

    pub(crate) fn worker_response_type_annotated_value(worker_response: &RefinedWorkerResponse) -> Option<TypeAnnotatedValue> {
        let typed_value = worker_response.to_type_annotated_value();

        typed_value.map(|typed_value| {
           let response =  create_record("response", typed_value);
            create_record("worker", response)
        })
    }

    pub(crate) fn create_record(name: &str, value: TypeAnnotatedValue) -> TypeAnnotatedValue {
        TypeAnnotatedValue::Record {
            typ: vec![(name.to_string(), AnalysedType::from(&value))],
            value: vec![(name.to_string(), value)].into_iter().collect(),
        }
    }
}