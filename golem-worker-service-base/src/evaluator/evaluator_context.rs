use golem_wasm_ast::analysis::AnalysedType;
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
}

impl EvaluationContext {
    pub fn empty() -> Self {
        EvaluationContext {
            variables: None,
        }
    }

    pub fn merge(&mut self, that: EvaluationContext) {
        if let Some(variables) = that.variables {
            self.merge_variables(&variables);
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
        let type_annoated_value = request.to_type_annotated_value();
        let variables = TypeAnnotatedValue::Record {
            typ: vec![("request".to_string(), AnalysedType::from(&type_annoated_value))],
            value: vec![("request".to_string(), type_annoated_value)].into_iter().collect(),
        };
        EvaluationContext {
            variables: Some(variables),
        }
    }

    pub fn from(
        worker_request: &WorkerRequest,
        worker_response: &RefinedWorkerResponse,
        request: &RequestDetails,
    ) -> Self {
        let inner =
            internal::worker_type_annotated_value(worker_response, worker_request);

        let variables = TypeAnnotatedValue::Record {
            typ: vec![("worker".to_string(), AnalysedType::from(&inner))],
            value: vec![("worker".to_string(), inner)].into_iter().collect(),
        };

        EvaluationContext {
            variables: Some(variables),
        }
    }
}

mod internal {
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerRequest};
    use crate::merge::Merge;

    pub(crate) fn worker_type_annotated_value(worker_response: &RefinedWorkerResponse, worker_request: &WorkerRequest) -> TypeAnnotatedValue {
        let mut typed_worker_data = worker_request.clone().to_type_annotated_value();

        if let Some(typed_res) = worker_response.to_type_annotated_value() {
            typed_worker_data.merge(&with_response_key(typed_res));
        }

        typed_worker_data
    }

    fn with_response_key(typed_res: TypeAnnotatedValue) -> TypeAnnotatedValue {
        TypeAnnotatedValue::Record {
            typ: vec![("response".to_string(), AnalysedType::from(&typed_res))],
            value: vec![("response".to_string(), typed_res)],
        }
    }
}