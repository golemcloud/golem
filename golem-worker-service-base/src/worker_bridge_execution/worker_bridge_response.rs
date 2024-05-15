
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;

use crate::worker_bridge_execution::{WorkerResponse};
// Refined Worker response is different from WorkerResponse, because,
// it ensures that we are not returning a vector of result if they are not named results
// or uni
#[derive(Debug, Clone)]
pub enum RefinedWorkerResponse {
    Unit,
    SingleResult(TypeAnnotatedValue),
    MultipleResults(TypeAnnotatedValue),
}

impl RefinedWorkerResponse {
    pub(crate) fn from_worker_response(
        worker_response: &WorkerResponse,
    ) -> Result<RefinedWorkerResponse, String> {
        let result = &worker_response.result.result;
        let function_result_types = &worker_response.result.function_result_types;

        if function_result_types.iter().all(|r| r.name.is_none())
            && !function_result_types.is_empty()
        {
            match result {
                TypeAnnotatedValue::Tuple { value, .. } => {
                    if value.len() == 1 {
                        Ok(RefinedWorkerResponse::SingleResult(value[0].clone()))
                    } else if value.is_empty() {
                        Ok(RefinedWorkerResponse::Unit)
                    } else {
                        Err(format!("Internal Error. WorkerBridge expects the result from worker to be a Tuple with 1 element if results are unnamed. Obtained {:?}", AnalysedType::from(result)))
                    }
                }
                ty => Err(format!("Internal Error. WorkerBridge expects the result from worker to be a Tuple if results ae unnamed. Obtained {:?}", AnalysedType::from(ty))),
            }
        } else {
            match &worker_response.result.result {
                TypeAnnotatedValue::Record { .. } => {
                    Ok(RefinedWorkerResponse::MultipleResults(worker_response.result.result.clone()))
                }

                // See wasm-rpc implementations for more details
                ty => Err(format!("Internal Error. WorkerBridge expects the result from worker to be a Record if results are named. Obtained {:?}", AnalysedType::from(ty))),
            }
        }
    }
}

