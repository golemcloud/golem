use crate::evaluator::evaluator_context::internal::create_record;
use crate::evaluator::getter::GetError;
use crate::evaluator::path::Path;
use crate::evaluator::Getter;
use crate::merge::Merge;
use crate::worker_binding::{RequestDetails, WorkerDetail};
use crate::worker_bridge_execution::RefinedWorkerResponse;
use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_wasm_ast::analysis::AnalysedFunction;
use golem_wasm_rpc::TypeAnnotatedValue;

#[derive(Clone)]
pub struct EvaluationContext {
    pub variables: Option<TypeAnnotatedValue>,
    pub analysed_functions: Vec<AnalysedFunction>,
}

#[async_trait]
pub trait WorkerMetadataFetcher {
    async fn get_worker_metadata(
        &self,
        component_id: &ComponentId,
    ) -> Result<Vec<AnalysedFunction>, MetadataFetchError>;
}

pub struct MetadataFetchError(pub String);

pub struct NoopWorkerMetadataFetcher;

#[async_trait]
impl WorkerMetadataFetcher for NoopWorkerMetadataFetcher {
    async fn get_worker_metadata(&self, _component_id: &ComponentId) -> Vec<AnalysedFunction> {
        vec![]
    }
}

impl EvaluationContext {
    pub fn empty() -> Self {
        EvaluationContext {
            variables: None,
            analysed_functions: vec![],
        }
    }

    pub fn merge(&mut self, that: &EvaluationContext) -> EvaluationContext {
        if let Some(that_variables) = &that.variables {
            self.merge_variables(that_variables);
        }

        self.clone()
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

    pub fn from_all(
        worker_detail: &WorkerDetail,
        request: &RequestDetails,
        functions: Vec<AnalysedFunction>,
    ) -> Self {
        let mut request_data = internal::request_type_annotated_value(&request);
        let worker_data = create_record("worker", worker_detail.clone().to_type_annotated_value());
        let merged = request_data.merge(&worker_data);

        EvaluationContext {
            variables: Some(merged.clone()),
            analysed_functions: functions,
        }
    }

    pub fn from_worker_detail(worker_detail: &WorkerDetail) -> Self {
        let typed_value = worker_detail.clone().to_type_annotated_value();
        let worker_data = create_record("worker", typed_value);

        EvaluationContext {
            variables: Some(worker_data),
            analysed_functions: vec![],
        }
    }

    pub fn from_request_data(request: &RequestDetails) -> Self {
        let variables = internal::request_type_annotated_value(request);

        EvaluationContext {
            variables: Some(variables),
            analysed_functions: vec![],
        }
    }

    #[allow(unused)]
    pub fn from_refined_worker_response(worker_response: &RefinedWorkerResponse) -> Self {
        let type_annoated_value = worker_response.to_type_annotated_value();

        if let Some(typed_res) = type_annoated_value {
            let response_data = internal::create_record("response", typed_res);
            let worker_data = internal::create_record("worker", response_data);

            EvaluationContext {
                variables: Some(worker_data),
                analysed_functions: vec![],
            }
        } else {
            EvaluationContext::empty()
        }
    }
}

mod internal {
    use crate::worker_bridge_execution::RefinedWorkerResponse;
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::TypeAnnotatedValue;

    use crate::worker_binding::RequestDetails;

    pub(crate) fn request_type_annotated_value(
        request_details: &RequestDetails,
    ) -> TypeAnnotatedValue {
        let type_annoated_value = request_details.to_type_annotated_value();
        create_record("request", type_annoated_value)
    }

    pub(crate) fn worker_response_type_annotated_value(
        worker_response: &RefinedWorkerResponse,
    ) -> Option<TypeAnnotatedValue> {
        let typed_value = worker_response.to_type_annotated_value();

        typed_value.map(|typed_value| {
            let response = create_record("response", typed_value);
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
