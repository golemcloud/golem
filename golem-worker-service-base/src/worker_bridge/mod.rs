use golem_wasm_rpc::json::get_json_from_typed_value;
use golem_wasm_rpc::TypeAnnotatedValue;
use serde_json::Value;

use golem_common::model::TemplateId;

use crate::evaluator::{Evaluator, RawString};
use crate::worker_binding::worker_binding_resolver::ResolvedWorkerBinding;

pub mod worker_request_executor;
pub mod worker_response;

// Every input request can be resolved to a worker request,
// along with the value of any variables that's associated with it.
#[derive(PartialEq, Debug, Clone)]
pub struct WorkerRequest {
    pub template: TemplateId,
    pub worker_id: String,
    pub function: String,
    pub function_params: Value,
}

impl WorkerRequest {
    // A worker-request can be formed from a route definition along with variables that were resolved using incoming http request
    pub fn from_resolved_route(
        resolved_route: ResolvedWorkerBinding,
    ) -> Result<WorkerRequest, String> {
        let worker_id_value: TypeAnnotatedValue = resolved_route
            .resolved_worker_binding_template
            .worker_id
            .evaluate(&resolved_route.typed_value_from_input)
            .map_err(|err| err.to_string())?;

        let worker_id = match worker_id_value {
            TypeAnnotatedValue::Str(value) => value,
            _ => {
                return Err(format!(
                    "Worker id is not a string. {}",
                    get_json_from_typed_value(&worker_id_value)
                ))
            }
        };

        let function_name_value = RawString::new(
            &resolved_route
                .resolved_worker_binding_template
                .function_name,
        )
        .evaluate(&resolved_route.typed_value_from_input)
        .map_err(|err| err.to_string())?;

        let function_name = match function_name_value {
            TypeAnnotatedValue::Str(value) => value,
            _ => {
                return Err(format!(
                    "Function name is not a string. {}",
                    get_json_from_typed_value(&function_name_value)
                ))
            }
        };

        let mut function_params: Vec<Value> = vec![];

        for expr in &resolved_route
            .resolved_worker_binding_template
            .function_params
        {
            let type_annotated_value = expr
                .evaluate(&resolved_route.typed_value_from_input)
                .map_err(|err| err.to_string())?;

            let json = get_json_from_typed_value(&type_annotated_value);

            function_params.push(json);
        }

        Ok(WorkerRequest {
            worker_id,
            template: resolved_route
                .resolved_worker_binding_template
                .template
                .clone(),
            function: function_name,
            function_params: Value::Array(function_params),
        })
    }
}
