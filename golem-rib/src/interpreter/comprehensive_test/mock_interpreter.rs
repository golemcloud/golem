#[cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;
use golem_wasm_ast::analysis::{AnalysedType, TypeStr};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::TypedTuple;
use crate::{Interpreter, RibFunctionInvoke};
use crate::interpreter::comprehensive_test::{mock_data, test_utils};
use crate::interpreter::env::InterpreterEnv;
use crate::interpreter::stack::InterpreterStack;


pub(crate) fn interpreter() -> Interpreter {
    let record_input_type = test_utils::analysed_type_record(vec![
        (
            "headers",
            test_utils::analysed_type_record(vec![("name", AnalysedType::Str(TypeStr))]),
        ),
        (
            "body",
            test_utils::analysed_type_record(vec![
                ("name", AnalysedType::Str(TypeStr)),
            ]),
        ),
        (
            "path",
            test_utils::analysed_type_record(vec![("name", AnalysedType::Str(TypeStr))]),
        ),
    ]);


    let functions_and_results: Vec<(&str, Option<TypeAnnotatedValue>)> =
        vec![
            ("function-unit-response", None),
            ("function-no-arg", Some(mock_data::str_data())),
            ("function-no-arg-unit", None),
            ("function-str-response", Some(mock_data::str_data())),
            ("function-number-response", Some(mock_data::number_data())),
            ("function-option-str-response", Some(mock_data::option_of_str())),
            ("function-option-number-response", Some(mock_data::option_of_number())),
            ("function-option-option-response", Some(mock_data::option_of_option())),
            ("function-option-variant-response", Some(mock_data::option_of_variant())),
            ("function-option-enum-response", Some(mock_data::option_of_enum())),
            ("function-option-tuple-response", Some(mock_data::option_of_tuple())),
            ("function-option-record-response", Some(mock_data::option_of_record())),
            ("function-option-list-response", Some(mock_data::option_of_list())),
            ("function-list-number-response", Some(mock_data::list_of_number())),
            ("function-list-str-response", Some(mock_data::list_of_str())),
            ("function-list-option-response", Some(mock_data::list_of_option())),
            ("function-list-list-response", Some(mock_data::list_of_list())),
            ("function-list-variant-response", Some(mock_data::list_of_variant())),
            ("function-list-enum-response", Some(mock_data::list_of_enum())),
            ("function-list-tuple-response", Some(mock_data::list_of_tuple())),
            ("function-list-record-response", Some(mock_data::list_of_record())),
            ("function-result-str-response", Some(mock_data::result_of_str())),
            ("function-result-number-response", Some(mock_data::result_of_number())),
            ("function-result-option-response", Some(mock_data::result_of_option())),
            ("function-result-variant-response", Some(mock_data::result_of_variant())),
            ("function-result-enum-response", Some(mock_data::result_of_enum())),
            ("function-result-tuple-response", Some(mock_data::result_of_tuple())),
            ("function-result-flag-response", Some(mock_data::result_of_flag())),
            ("function-result-record-response", Some(mock_data::result_of_record())),
            ("function-result-list-response", Some(mock_data::result_of_list())),
            ("function-tuple-response", Some(mock_data::tuple())),
            ("function-enum-response", Some(mock_data::enum_data())),
            ("function-flag-response", Some(mock_data::flag())),
            ("function-variant-response", Some(mock_data::variant())),
            ("function-record-response", Some(mock_data::record())),
            ("function-all-inputs", Some(mock_data::str_data()))
        ];

    let functions_and_result: HashMap<FunctionName, Option<TypeAnnotatedValue>> = functions_and_results
        .into_iter()
        .map(|(name, result)| (FunctionName(name.to_string()), result))
        .collect();

    let interpreter_env_input: HashMap<String, TypeAnnotatedValue> = HashMap::new();

    dynamic_test_interpreter(functions_and_result, interpreter_env_input)
}


#[derive(Clone, Hash, PartialEq, Eq)]
struct FunctionName(pub(crate) String);


fn dynamic_test_interpreter(
    functions_and_result: HashMap<FunctionName, Option<TypeAnnotatedValue>>,
    interpreter_env_input: HashMap<String, TypeAnnotatedValue>,
) -> Interpreter {
    Interpreter {
        stack: InterpreterStack::default(),
        env: InterpreterEnv::from(
            interpreter_env_input,
            dynamic_worker_invoke(functions_and_result),
        ),
    }
}

fn dynamic_worker_invoke(
    functions_and_result: HashMap<FunctionName, Option<TypeAnnotatedValue>>,
) -> RibFunctionInvoke {
    let value = functions_and_result.clone();

    Arc::new(move |a, _| {
        Box::pin({
            let value = value.get(&FunctionName(a)).cloned().flatten();
            let analysed_type = value.clone().map(|x| AnalysedType::try_from(&x).unwrap());

            async move {
                let analysed_type = analysed_type.clone();
                let value = value.clone();

                if let Some(value) = value {
                    Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                        typ: vec![golem_wasm_ast::analysis::protobuf::Type::from(
                            &analysed_type.unwrap(),
                        )],
                        value: vec![golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                            type_annotated_value: Some(value),
                        }],
                    }))
                } else {
                    // Representing Unit
                    Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                        typ: vec![],
                        value: vec![],
                    }))
                }
            }
        })
    })
}