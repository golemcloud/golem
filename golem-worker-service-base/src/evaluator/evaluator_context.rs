use std::fmt::{Debug, Display, Formatter};

use async_trait::async_trait;
use golem_wasm_rpc::TypeAnnotatedValue;

use golem_common::model::function_name::ParsedFunctionName;
use golem_service_base::model::{ComponentMetadata, FunctionParameter, FunctionResult, WorkerId};

use crate::evaluator::evaluator_context::internal::create_record;
use crate::evaluator::getter::GetError;
use crate::evaluator::path::Path;
use crate::evaluator::Getter;
use crate::merge::Merge;
use crate::worker_binding::{RequestDetails, WorkerDetail};
use crate::worker_bridge_execution::RefinedWorkerResponse;

#[derive(Debug, Clone)]
pub struct EvaluationContext {
    pub variables: Option<TypeAnnotatedValue>,
    pub functions: Vec<Function>,
}

#[derive(PartialEq, Debug, Clone)]
pub struct FQN {
    pub parsed_function_name: ParsedFunctionName,
}

impl TryFrom<&str> for FQN {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let parsed_function_name = ParsedFunctionName::parse(value)?;

        Ok(FQN {
            parsed_function_name,
        })
    }
}

impl Display for FQN {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let result = self.parsed_function_name.clone();
        let site = result.site();
        let site_str = site.interface_name();
        let func_ref = result.function();
        let function_name = func_ref.function_name();
        let name = site_str.map_or(function_name.clone(), |s| {
            format!("{}.{{{}}}", s.clone(), function_name)
        });
        write!(f, "{}", name)
    }
}

#[derive(Debug, Clone)]
pub struct Function {
    pub fqn: FQN,
    pub arguments: Vec<FunctionParameter>,
    pub return_type: Vec<FunctionResult>,
}

#[async_trait]
pub trait WorkerMetadataFetcher {
    async fn get_worker_metadata(
        &self,
        worker_id: &WorkerId,
    ) -> Result<ComponentMetadata, MetadataFetchError>;
}

pub struct MetadataFetchError(pub String);

impl Display for MetadataFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Worker component metadata fetch error: {}", self.0)
    }
}

pub struct NoopWorkerMetadataFetcher;

#[async_trait]
impl WorkerMetadataFetcher for NoopWorkerMetadataFetcher {
    async fn get_worker_metadata(
        &self,
        _worker_id: &WorkerId,
    ) -> Result<ComponentMetadata, MetadataFetchError> {
        Ok(ComponentMetadata {
            exports: vec![],
            producers: vec![],
            memories: vec![],
        })
    }
}

impl EvaluationContext {
    pub fn empty() -> Self {
        EvaluationContext {
            variables: None,
            functions: vec![],
        }
    }

    pub fn find_function(&self, function: &str) -> Result<Option<Function>, String> {
        let fqn = FQN::try_from(function)?;
        Ok(self.functions.iter().find(|f| f.fqn == fqn).cloned())
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
        component_metadata: ComponentMetadata,
    ) -> Result<Self, String> {
        let mut request_data = internal::request_type_annotated_value(request);
        let worker_data = create_record("worker", worker_detail.clone().to_type_annotated_value());
        let merged = request_data.merge(&worker_data);

        let top_level_functions = component_metadata.functions();

        let functions = top_level_functions
            .iter()
            .map(|f| Function {
                fqn: FQN {
                    parsed_function_name: ParsedFunctionName::global(f.name.clone()),
                },
                arguments: f.parameters.clone(),
                return_type: f.results.clone(),
            })
            .collect::<Vec<Function>>();

        let function_of_interfaces = component_metadata
            .instances()
            .iter()
            .flat_map(|i| {
                i.functions.iter().map(move |f| Function {
                    fqn: FQN {
                        parsed_function_name: ParsedFunctionName::parse(format!(
                            "{}.{{{}}}",
                            i.name.clone(),
                            f.name.clone()
                        ))
                        .unwrap(),
                    },
                    arguments: f.parameters.clone(),
                    return_type: f.results.clone(),
                })
            })
            .collect::<Vec<Function>>();

        Ok(EvaluationContext {
            variables: Some(merged.clone()),
            functions: function_of_interfaces
                .into_iter()
                .chain(functions)
                .collect(),
        })
    }

    pub fn from_worker_detail(worker_detail: &WorkerDetail) -> Self {
        let typed_value = worker_detail.clone().to_type_annotated_value();
        let worker_data = create_record("worker", typed_value);

        EvaluationContext {
            variables: Some(worker_data),
            functions: vec![],
        }
    }

    pub fn from_request_data(request: &RequestDetails) -> Self {
        let variables = internal::request_type_annotated_value(request);

        EvaluationContext {
            variables: Some(variables),
            functions: vec![],
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
                functions: vec![],
            }
        } else {
            EvaluationContext::empty()
        }
    }
}

mod internal {
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::TypeAnnotatedValue;

    use crate::worker_binding::RequestDetails;

    pub(crate) fn request_type_annotated_value(
        request_details: &RequestDetails,
    ) -> TypeAnnotatedValue {
        let type_annoated_value = request_details.to_type_annotated_value();
        create_record("request", type_annoated_value)
    }

    pub(crate) fn create_record(name: &str, value: TypeAnnotatedValue) -> TypeAnnotatedValue {
        TypeAnnotatedValue::Record {
            typ: vec![(name.to_string(), AnalysedType::from(&value))],
            value: vec![(name.to_string(), value)].into_iter().collect(),
        }
    }
}
