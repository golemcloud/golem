use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
    AnalysedType, NameTypePair, TypeList, TypeOption, TypeRecord, TypeStr,
};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::TypedOption;
use rib::{Expr, FunctionTypeRegistry};

pub(crate) fn get_analysed_type_record(record_type: Vec<(String, AnalysedType)>) -> AnalysedType {
    let record = TypeRecord {
        fields: record_type
            .into_iter()
            .map(|(name, typ)| NameTypePair { name, typ })
            .collect(),
    };
    AnalysedType::Record(record)
}

pub(crate) fn create_none(typ: Option<&AnalysedType>) -> TypeAnnotatedValue {
    TypeAnnotatedValue::Option(Box::new(TypedOption {
        value: None,
        typ: typ.map(|t| t.into()),
    }))
}

pub(crate) fn get_analysed_exports(
    function_name: &str,
    input_types: Vec<AnalysedType>,
    output: AnalysedType,
) -> Vec<AnalysedExport> {
    let analysed_function_parameters = input_types
        .into_iter()
        .enumerate()
        .map(|(index, typ)| AnalysedFunctionParameter {
            name: format!("param{}", index),
            typ,
        })
        .collect();

    vec![AnalysedExport::Function(AnalysedFunction {
        name: function_name.to_string(),
        parameters: analysed_function_parameters,
        results: vec![AnalysedFunctionResult {
            name: None,
            typ: output,
        }],
    })]
}

fn main() {
    let request_body_type = get_analysed_type_record(vec![
        ("id".to_string(), AnalysedType::Str(TypeStr)),
        ("name".to_string(), AnalysedType::Str(TypeStr)),
        (
            "titles".to_string(),
            AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::Str(TypeStr)),
            }),
        ),
        (
            "address".to_string(),
            get_analysed_type_record(vec![
                ("street".to_string(), AnalysedType::Str(TypeStr)),
                ("city".to_string(), AnalysedType::Str(TypeStr)),
            ]),
        ),
    ]);

    let worker_response = create_none(Some(&AnalysedType::Str(TypeStr)));

    let request_type =
        get_analysed_type_record(vec![("body".to_string(), request_body_type.clone())]);
    // Output from worker

    let return_type = AnalysedType::Option(TypeOption {
        inner: Box::new(AnalysedType::try_from(&worker_response).unwrap()),
    });

    let component_metadata = get_analysed_exports("foo", vec![request_type.clone()], return_type);

    let expr_str = r#"${

             let x = { body : { id: "bId", name: "bName", titles: request.body.titles, address: request.body.address } };
              let result = foo(x);
              match result {  some(value) => "personal-id", none =>  x.body.titles[1] }
            }"#;

    let mut expr = Expr::from_interpolated_str(expr_str).unwrap();

    let function_type_registry = FunctionTypeRegistry::from_export_metadata(&component_metadata);

    expr.infer_types(&function_type_registry).unwrap();

    dbg!(expr.clone());
}
