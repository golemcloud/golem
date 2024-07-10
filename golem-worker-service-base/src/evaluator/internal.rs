
        use golem_wasm_rpc::protobuf::TypedRecord;
        use golem_wasm_rpc::{get_analysed_type, TypeAnnotatedValue, TypeExt};
        use golem_wasm_rpc::protobuf::NameValuePair;
        use golem_wasm_rpc::protobuf::NameTypePair;
        use crate::evaluator::EvaluationError;
        use crate::evaluator::getter::Getter;
        use crate::evaluator::path::Path;
        use crate::evaluator::EvaluationContext;
        use crate::primitive::GetPrimitive;
        use crate::worker_bridge_execution::{
            RefinedWorkerResponse, WorkerRequest, WorkerRequestExecutor,
        };
        use golem_common::model::{ComponentId, IdempotencyKey};
        use rib::ParsedFunctionName;
        use std::str::FromStr;
        use std::sync::Arc;
        pub(crate) fn create_record(binding_variable: &str,  value: &TypeAnnotatedValue) -> Result<TypeAnnotatedValue, EvaluationError> {
            let name_value_pair = NameValuePair {
                name: binding_variable.to_string(),
                value: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue { type_annotated_value:  Some(value.clone()) }),
            };

            let typ =
                get_analysed_type(value).map_err(|_| EvaluationError::Message("Failed to get analysed type".to_string()))?;

            let name_type_pair = NameTypePair {
                name: binding_variable.to_string(),
                typ: Some(typ.to_type()),
            };

            Ok(TypeAnnotatedValue::Record (TypedRecord{
                value: vec![name_value_pair],
                typ: vec![name_type_pair],
            }))
        }

        pub(crate) async fn call_worker_function(
            runtime: &EvaluationContext,
            function_name: &ParsedFunctionName,
            json_params: Vec<TypeAnnotatedValue>,
            executor: &Arc<dyn WorkerRequestExecutor + Sync + Send>,
        ) -> Result<RefinedWorkerResponse, EvaluationError> {
            let variables = runtime.clone().variables.ok_or(EvaluationError::Message(
                "No variables found in the context".to_string(),
            ))?;

            let worker_variables = variables.get(&Path::from_key("worker")).map_err(|_| {
                EvaluationError::Message("No worker variables found in the context".to_string())
            })?;

            let worker_name_typed = worker_variables.get(&Path::from_key("name")).map_err(|_| {
                EvaluationError::Message("No worker name found in the context".to_string())
            })?;

            let worker_name = worker_name_typed
                .get_primitive()
                .ok_or(EvaluationError::Message(
                    "Worker name is not a string".to_string(),
                ))?
                .as_string();

            let idempotency_key = worker_variables
                .get(&Path::from_key("idempotency-key"))
                .ok()
                .and_then(|v| v.get_primitive())
                .map(|p| IdempotencyKey::new(p.as_string()));

            let component_id = worker_variables
                .get(&Path::from_key("component_id"))
                .map_err(|_| {
                    EvaluationError::Message("No component_id found in the context".to_string())
                })?;

            let component_id_string = component_id
                .get_primitive()
                .ok_or(EvaluationError::Message(
                    "Component_id is not a string".to_string(),
                ))?
                .as_string();

            let component_id = ComponentId::from_str(component_id_string.as_str())
                .map_err(|err| EvaluationError::Message(err.to_string()))?;

            let worker_request = WorkerRequest {
                component_id,
                worker_name,
                function_name: function_name.clone(),
                function_params: json_params,
                idempotency_key,
            };

            let worker_response = executor.execute(worker_request).await.map_err(|err| {
                EvaluationError::Message(format!("Failed to execute worker function: {}", err))
            })?;

            let refined_worker_response = worker_response.refined().map_err(|err| {
                EvaluationError::Message(format!("Failed to refine worker response: {}", err))
            })?;

            Ok(refined_worker_response)
        }