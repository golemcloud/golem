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
        let result = &worker_response.result;
        match result {
            TypeAnnotatedValue::Tuple(TypedTuple { value, .. }) if value.is_empty() => {
                Ok(RefinedWorkerResponse::Unit)
            }
            TypeAnnotatedValue::Tuple(TypedTuple { value, .. }) if value.len() == 1 => {
                let inner = value[0]
                    .clone()
                    .type_annotated_value
                    .ok_or("Internal Error. Unexpected empty result")?;
                Ok(RefinedWorkerResponse::SingleResult(inner))
            }
            ty => Ok(RefinedWorkerResponse::MultipleResults(ty.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::service::worker::TypedResult;
    use golem_service_base::model::{FunctionResult, Type, TypeU32};
    use golem_wasm_rpc::get_type;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::{NameTypePair, NameValuePair, TypedRecord, TypedTuple};

    use crate::worker_bridge_execution::refined_worker_response::RefinedWorkerResponse;
    use crate::worker_bridge_execution::WorkerResponse;

    fn create_record(values: Vec<(String, TypeAnnotatedValue)>) -> TypeAnnotatedValue {
        let mut name_type_pairs = vec![];
        let mut name_value_pairs = vec![];

        for (key, value) in values.iter() {
            let typ = get_type(value).unwrap();
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

        TypeAnnotatedValue::Record(TypedRecord {
            typ: name_type_pairs,
            value: name_value_pairs,
        })
    }

    fn create_tuple(value: Vec<TypeAnnotatedValue>) -> TypeAnnotatedValue {
        let mut types = vec![];

        for value in value.iter() {
            let typ = get_type(value).unwrap();
            types.push(typ);
        }

        TypeAnnotatedValue::Tuple(TypedTuple {
            value: value
                .into_iter()
                .map(|result| golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(result.clone()),
                })
                .collect(),
            typ: types,
        })
    }

    #[test]
    fn test_refined_worker_response_from_worker_response() {
        let worker_response = WorkerResponse {
            result: create_tuple(vec![TypeAnnotatedValue::U32(1)]),
        };

        let refined_worker_response =
            RefinedWorkerResponse::from_worker_response(&worker_response).unwrap();
        assert_eq!(
            refined_worker_response,
            RefinedWorkerResponse::SingleResult(TypeAnnotatedValue::U32(1))
        );

        let worker_response = WorkerResponse {
            result: create_tuple(vec![]),
        };

        let refined_worker_response =
            RefinedWorkerResponse::from_worker_response(&worker_response).unwrap();
        assert_eq!(refined_worker_response, RefinedWorkerResponse::Unit);

        let worker_response = WorkerResponse {
            result: create_record(vec![("foo".to_string(), TypeAnnotatedValue::U32(1))]),
        };

        let refined_worker_response =
            RefinedWorkerResponse::from_worker_response(&worker_response).unwrap();
        assert_eq!(
            refined_worker_response,
            RefinedWorkerResponse::MultipleResults(create_record(vec![(
                "foo".to_string(),
                TypeAnnotatedValue::U32(1)
            )]))
        );
    }
}
