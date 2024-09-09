use golem_wasm_ast::analysis::AnalysedFunctionResult;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{protobuf, TypeAnnotatedValueConstructors};

pub fn interpret_function_results(
    function_results: Vec<golem_wasm_rpc::Value>,
    expected_types: Vec<AnalysedFunctionResult>,
) -> Result<TypeAnnotatedValue, Vec<String>> {
    if function_results.len() != expected_types.len() {
        Err(vec![format!(
            "Unexpected number of result values (got {}, expected: {})",
            function_results.len(),
            expected_types.len()
        )])
    } else {
        let mut results = vec![];
        let mut errors = vec![];

        for (value, expected) in function_results.iter().zip(expected_types.iter()) {
            let analysed_typ = &expected.typ;
            let result = TypeAnnotatedValue::create(value, analysed_typ);

            match result {
                Ok(value) => {
                    results.push((value, analysed_typ.into()));
                }
                Err(err) => errors.extend(err),
            }
        }

        let all_without_names = expected_types.iter().all(|t| t.name.is_none());

        if all_without_names {
            let mut types = vec![];
            let mut values = vec![];

            for (value, typ) in results {
                types.push(typ);
                values.push(protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(value),
                });
            }

            let tuple = protobuf::TypedTuple {
                typ: types,
                value: values,
            };

            Ok(TypeAnnotatedValue::Tuple(tuple))
        } else {
            let mut named_typs: Vec<(String, protobuf::Type)> = vec![];
            let mut named_values: Vec<(String, TypeAnnotatedValue)> = vec![];

            for (index, ((typed_value, typ), expected)) in
                results.into_iter().zip(expected_types.iter()).enumerate()
            {
                let name = if let Some(name) = &expected.name {
                    name.clone()
                } else {
                    index.to_string()
                };

                named_typs.push((name.clone(), typ.clone()));
                named_values.push((name, typed_value));
            }

            let record = protobuf::TypedRecord {
                typ: named_typs
                    .iter()
                    .map(|(name, typ)| protobuf::NameTypePair {
                        name: name.clone(),
                        typ: Some(typ.clone()),
                    })
                    .collect(),
                value: named_values
                    .iter()
                    .map(|(name, value)| protobuf::NameValuePair {
                        name: name.clone(),
                        value: Some(protobuf::TypeAnnotatedValue {
                            type_annotated_value: Some(value.clone()),
                        }),
                    })
                    .collect(),
            };

            Ok(TypeAnnotatedValue::Record(record))
        }
    }
}
