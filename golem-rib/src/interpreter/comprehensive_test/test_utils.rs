#[cfg(test)]

use golem_wasm_ast::analysis::*;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

pub(crate) fn analysed_type_record(fields: Vec<(&str, AnalysedType)>) -> AnalysedType {
    AnalysedType::Record(TypeRecord {
        fields: fields
            .into_iter()
            .map(|(name, typ)| NameTypePair {
                name: name.to_string(),
                typ,
            })
            .collect(),
    })
}

pub(crate) fn get_type_annotated_value(
    analysed_type: &AnalysedType,
    wasm_wave_str: &str,
) -> TypeAnnotatedValue {

    let result = golem_wasm_rpc::type_annotated_value_from_str(analysed_type, wasm_wave_str);

    match result {
        Ok(value) => value,
        Err(err) => panic!("Wasm wave syntax error {:?} {} {}", analysed_type, wasm_wave_str, err),
    }
}

pub(crate) fn convert_type_annotated_value_to_str(
    type_annotated_value: &TypeAnnotatedValue,
) -> String {
    golem_wasm_rpc::type_annotated_value_to_string(type_annotated_value).unwrap()
}

pub(crate) fn get_function_component_metadata(
    function_name: &str,
    input_types: Vec<AnalysedType>,
    output: Option<AnalysedType>,
) -> Vec<AnalysedExport> {
    let analysed_function_parameters = input_types
        .into_iter()
        .enumerate()
        .map(|(index, typ)| AnalysedFunctionParameter {
            name: format!("param{}", index),
            typ,
        })
        .collect();

    let results = if let Some(output) = output {
        vec![AnalysedFunctionResult {
            name: None,
            typ: output,
        }]
    } else {
        // Representing Unit
        vec![]
    };

    vec![AnalysedExport::Function(AnalysedFunction {
        name: function_name.to_string(),
        parameters: analysed_function_parameters,
        results,
    })]
}