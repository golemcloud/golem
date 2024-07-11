use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::get_analysed_type;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::TypedTuple;

use crate::worker_bridge_execution::WorkerResponse;

// Refined Worker response is different from WorkerResponse, because,
// it ensures that we are not returning a vector of result if they are not named results
// or unit
#[derive(Debug, Clone, PartialEq)]
pub enum RefinedWorkerResponse {
    Unit,
    SingleResult(TypeAnnotatedValue),
    MultipleResults(TypeAnnotatedValue),
}

impl RefinedWorkerResponse {
    pub(crate) fn to_type_annotated_value(&self) -> Option<TypeAnnotatedValue> {
        match self {
            RefinedWorkerResponse::Unit => None,
            RefinedWorkerResponse::SingleResult(value) => Some(value.clone()),
            RefinedWorkerResponse::MultipleResults(results) => Some(results.clone()),
        }
    }

    pub(crate) fn from_worker_response(
        worker_response: &WorkerResponse,
    ) -> Result<RefinedWorkerResponse, String> {
        let result = &worker_response.result.result;
        let function_result_types = &worker_response.result.function_result_types;

        if function_result_types.iter().all(|r| r.name.is_none()) {
            match result {
                TypeAnnotatedValue::Tuple (TypedTuple { value, .. } )=> {
                    if value.len() == 1 {
                        let inner = value[0].clone().type_annotated_value.ok_or("Internal Error. WorkerBridge expects the result from worker to be a Tuple with 1 element if results are unnamed. Obtained None")?;
                        Ok(RefinedWorkerResponse::SingleResult(inner))
                    } else if value.is_empty() {
                        Ok(RefinedWorkerResponse::Unit)
                    } else {
                        Err(format!("Internal Error. WorkerBridge expects the result from worker to be a Tuple with 1 element if results are unnamed. Obtained {:?}", get_analysed_type(result).ok()))
                    }
                }
                ty => Err(format!("Internal Error. WorkerBridge expects the result from worker to be a Tuple if results are unnamed. Obtained {:?}", get_analysed_type(ty).ok())),
            }
        } else {
            match &worker_response.result.result {
                TypeAnnotatedValue::Record { .. } => {
                    Ok(RefinedWorkerResponse::MultipleResults(worker_response.result.result.clone()))
                }

                // See wasm-rpc implementations for more details
                ty => Err(format!("Internal Error. WorkerBridge expects the result from worker to be a Record if results are named. Obtained {:?}",get_analysed_type(ty).ok())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::service::worker::TypedResult;
    use golem_service_base::model::{FunctionResult, Type, TypeU32};
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

    use crate::worker_bridge_execution::refined_worker_response::RefinedWorkerResponse;
    use crate::worker_bridge_execution::WorkerResponse;

    #[test]
    fn test_refined_worker_response_from_worker_response() {
        let worker_response = WorkerResponse {
            result: TypedResult {
                result: TypeAnnotatedValue::Tuple {
                    value: vec![TypeAnnotatedValue::U32(1)],
                    typ: vec![AnalysedType::U32],
                },
                function_result_types: vec![FunctionResult {
                    name: None,
                    typ: Type::U32(TypeU32),
                }],
            },
        };

        let refined_worker_response =
            RefinedWorkerResponse::from_worker_response(&worker_response).unwrap();
        assert_eq!(
            refined_worker_response,
            RefinedWorkerResponse::SingleResult(TypeAnnotatedValue::U32(1))
        );

        let worker_response = WorkerResponse {
            result: TypedResult {
                result: TypeAnnotatedValue::Tuple {
                    value: vec![],
                    typ: vec![],
                },
                function_result_types: vec![],
            },
        };

        let refined_worker_response =
            RefinedWorkerResponse::from_worker_response(&worker_response).unwrap();
        assert_eq!(refined_worker_response, RefinedWorkerResponse::Unit);

        let worker_response = WorkerResponse {
            result: TypedResult {
                result: TypeAnnotatedValue::Record {
                    typ: vec![("foo".to_string(), AnalysedType::U32)],
                    value: vec![("foo".to_string(), TypeAnnotatedValue::U32(1))],
                },
                function_result_types: vec![FunctionResult {
                    name: Some("name".to_string()),
                    typ: Type::U32(TypeU32),
                }],
            },
        };

        let refined_worker_response =
            RefinedWorkerResponse::from_worker_response(&worker_response).unwrap();
        assert_eq!(
            refined_worker_response,
            RefinedWorkerResponse::MultipleResults(TypeAnnotatedValue::Record {
                typ: vec![("foo".to_string(), AnalysedType::U32)],
                value: vec![("foo".to_string(), TypeAnnotatedValue::U32(1))],
            })
        );
    }
}
