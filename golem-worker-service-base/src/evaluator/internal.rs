use crate::evaluator::getter::Getter;
use crate::evaluator::path::Path;
use crate::evaluator::EvaluationContext;
use crate::evaluator::EvaluationError;
use crate::primitive::GetPrimitive;
use crate::worker_bridge_execution::{RefinedWorkerResponse, WorkerRequest, WorkerRequestExecutor};
use golem_common::model::{ComponentId, IdempotencyKey};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::protobuf::typed_result::ResultValue;
use golem_wasm_rpc::protobuf::{NameTypePair, TypedFlags, TypedTuple};
use golem_wasm_rpc::protobuf::NameValuePair;
use golem_wasm_rpc::protobuf::{TypedList, TypedOption, TypedRecord, TypedResult};
use golem_wasm_rpc::{get_analysed_type, get_type, TypeExt};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::TypeAnnotatedValue as RootTypeAnnotatedValue;
use rib::ParsedFunctionName;
use std::str::FromStr;
use std::sync::Arc;


pub(crate) fn create_tuple(
    value: Vec<TypeAnnotatedValue>
) -> Result<TypeAnnotatedValue, EvaluationError> {

    let mut types = vec![];

    for value in value.iter() {
        let typ = get_type(value).map_err(|_| EvaluationError::Message("Failed to get type".to_string()))?;
        types.push(typ);
    }

    Ok(TypeAnnotatedValue::Tuple(TypedTuple {
        value: value
            .into_iter()
            .map(|result| golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(result.clone()),
            })
            .collect(),
        typ: types
    }))
}

pub(crate) fn create_flags(value: Vec<String>) -> TypeAnnotatedValue {
    TypeAnnotatedValue::Flags(TypedFlags {
        values: value.clone(),
        typ: value.clone(),
    })
}

pub(crate) fn create_ok_result(
    value: TypeAnnotatedValue,
) -> Result<TypeAnnotatedValue, EvaluationError> {
    let analysed_type = get_type(&value)
        .map_err(|_| EvaluationError::Message("Failed to get analysed type".to_string()))?;
    let typed_value = TypeAnnotatedValue::Result(Box::new(TypedResult {
        result_value: Some(ResultValue::OkValue(Box::new(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
            type_annotated_value: Some(value),
        }))),
        ok: Some(analysed_type),
        error: None,
    }));

    Ok(typed_value)
}

pub(crate) fn create_error_result(
    value: TypeAnnotatedValue,
) -> Result<TypeAnnotatedValue, EvaluationError> {
    let analysed_type = get_type(&value)
        .map_err(|_| EvaluationError::Message("Failed to get analysed type".to_string()))?;
    let typed_value = TypeAnnotatedValue::Result(Box::new(TypedResult {
        result_value: Some(ResultValue::ErrorValue(Box::new(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
            type_annotated_value: Some(value),
        }))),
        ok: None,
        error: Some(analysed_type),
    }));

    Ok(typed_value)
}

pub(crate) fn create_option(
    value: TypeAnnotatedValue,
) -> Result<TypeAnnotatedValue, EvaluationError> {
    let typ = get_type(&value)
        .map_err(|_| EvaluationError::Message("Failed to get analysed type".to_string()))?;

    Ok(TypeAnnotatedValue::Option(Box::new(TypedOption {
        value: Some(Box::new(RootTypeAnnotatedValue {
            type_annotated_value: Some(value),
        })),
        typ: Some(typ),
    })))
}

pub(crate) fn create_none(typ: &AnalysedType) -> TypeAnnotatedValue {
    TypeAnnotatedValue::Option(Box::new(TypedOption {
        value: None,
        typ: Some(typ.to_type()),
    }))
}

pub(crate) fn create_list(
    values: Vec<TypeAnnotatedValue>,
) -> Result<TypeAnnotatedValue, EvaluationError> {
    match values.first() {
        Some(value) => {
            let typ = get_type(value)
                .map_err(|_| EvaluationError::Message("Failed to get analysed type".to_string()))?;

            Ok(TypeAnnotatedValue::List(TypedList {
                values: values
                    .into_iter()
                    .map(|result| RootTypeAnnotatedValue {
                        type_annotated_value: Some(result.clone()),
                    })
                    .collect(),
                typ: Some(typ),
            }))
        }
        None => Ok(TypeAnnotatedValue::List(TypedList {
            values: vec![],
            typ: Some(AnalysedType::Tuple(vec![]).to_type()),
        })),
    }
}
pub(crate) fn create_singleton_record(
    binding_variable: &str,
    value: &TypeAnnotatedValue,
) -> Result<TypeAnnotatedValue, EvaluationError> {
    create_record(vec![(binding_variable.to_string(), value.clone())])
}

pub(crate) fn create_record(
    values: Vec<(String, TypeAnnotatedValue)>,
) -> Result<TypeAnnotatedValue, EvaluationError> {
    let mut name_type_pairs = vec![];
    let mut name_value_pairs = vec![];

    for (key, value) in values.iter() {
        let typ = get_type(value)
            .map_err(|_| EvaluationError::Message("Failed to get type".to_string()))?;
        name_type_pairs.push(NameTypePair {
            name: key.to_string(),
            typ: Some(typ),
        });

        name_value_pairs.push(NameValuePair {
            name: key.to_string(),
            value: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(value.clone()),
            }),
        });
    }

    Ok(TypeAnnotatedValue::Record(TypedRecord {
        typ: name_type_pairs,
        value: name_value_pairs,
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

    let worker_name_typed = worker_variables
        .get(&Path::from_key("name"))
        .map_err(|_| EvaluationError::Message("No worker name found in the context".to_string()))?;

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

pub(crate) fn print_type(value: &TypeAnnotatedValue) -> String {
    get_analysed_type(value).map_or("<Unable to Resolve Type Info>".to_string(), |typ| format!("{:?}", typ))
}