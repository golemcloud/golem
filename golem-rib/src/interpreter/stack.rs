use crate::interpreter::result::RibInterpreterResult;
use golem_wasm_ast::analysis::protobuf::{NameTypePair, Type};
use golem_wasm_ast::analysis::{AnalysedType, NameOptionTypePair};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::{TypedList, TypedOption, TypedRecord, TypedTuple, TypedVariant};

#[derive(Debug)]
pub struct InterpreterStack {
    pub stack: Vec<RibInterpreterResult>,
}

impl Default for InterpreterStack {
    fn default() -> Self {
        Self::new()
    }
}

impl InterpreterStack {
    pub fn new() -> Self {
        InterpreterStack { stack: Vec::new() }
    }

    // Initialise a record in the stack
    pub fn create_record(&mut self, analysed_type: Vec<NameTypePair>) {
        self.push_val(TypeAnnotatedValue::Record(TypedRecord {
            value: vec![],
            typ: analysed_type,
        }));
    }

    pub fn create_list(&mut self, analysed_type: AnalysedType) {
        self.push_val(TypeAnnotatedValue::List(TypedList {
            values: vec![],
            typ: Some(Type::from(&analysed_type)),
        }));
    }

    pub fn pop(&mut self) -> Option<RibInterpreterResult> {
        self.stack.pop()
    }

    pub fn pop_n(&mut self, n: usize) -> Option<Vec<RibInterpreterResult>> {
        let mut results = Vec::new();
        for _ in 0..n {
            results.push(self.stack.pop()?);
        }
        Some(results)
    }

    pub fn pop_val(&mut self) -> Option<TypeAnnotatedValue> {
        self.stack.pop().and_then(|v| v.get_val())
    }

    pub fn push(&mut self, interpreter_result: RibInterpreterResult) {
        self.stack.push(interpreter_result);
    }

    pub fn push_val(&mut self, element: TypeAnnotatedValue) {
        self.stack.push(RibInterpreterResult::val(element));
    }

    pub fn push_variant(
        &mut self,
        variant_name: String,
        optional_variant_value: Option<TypeAnnotatedValue>,
        typ: Vec<NameOptionTypePair>,
    ) {
        // The GRPC issues
        let optional_type_annotated_value = optional_variant_value.map(|type_value| {
            Box::new(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(type_value),
            })
        });

        let value = TypeAnnotatedValue::Variant(Box::new(TypedVariant {
            case_name: variant_name.clone(),
            case_value: optional_type_annotated_value,
            typ: Some(golem_wasm_ast::analysis::protobuf::TypeVariant {
                cases: typ
                    .into_iter()
                    .map(
                        |name| golem_wasm_ast::analysis::protobuf::NameOptionTypePair {
                            name: name.name,
                            typ: name
                                .typ
                                .map(|x| golem_wasm_ast::analysis::protobuf::Type::from(&x)),
                        },
                    )
                    .collect(),
            }),
        }));

        self.push_val(value);
    }

    pub fn push_some(&mut self, inner_element: TypeAnnotatedValue, inner_type: &AnalysedType) {
        self.push_val(TypeAnnotatedValue::Option(Box::new(TypedOption {
            typ: Some(golem_wasm_ast::analysis::protobuf::Type::from(inner_type)),
            value: Some(Box::new(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                type_annotated_value: Some(inner_element),
            })),
        })));
    }

    // We allow untyped none to be in stack,
    // Need to verify how strict we should be
    // Example: ${match ok(1) { ok(value) => none }} should be allowed
    pub fn push_none(&mut self, analysed_type: Option<AnalysedType>) {
        self.push_val(TypeAnnotatedValue::Option(Box::new(TypedOption {
            typ: analysed_type.map(|x| golem_wasm_ast::analysis::protobuf::Type::from(&x)),
            value: None,
        })));
    }

    pub fn push_ok(
        &mut self,
        inner_element: TypeAnnotatedValue,
        ok_type: Option<&AnalysedType>,
        err_type: Option<&AnalysedType>,
    ) {
        let ok_type = golem_wasm_ast::analysis::protobuf::Type::from(
            ok_type.unwrap_or(&AnalysedType::try_from(&inner_element).unwrap()),
        );

        self.push_val(TypeAnnotatedValue::Result(Box::new(
            golem_wasm_rpc::protobuf::TypedResult {
                result_value: Some(
                    golem_wasm_rpc::protobuf::typed_result::ResultValue::OkValue(Box::new(
                        golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                            type_annotated_value: Some(inner_element),
                        },
                    )),
                ),
                ok: Some(ok_type),
                error: err_type.map(golem_wasm_ast::analysis::protobuf::Type::from),
            },
        )));
    }

    pub fn push_err(
        &mut self,
        inner_element: TypeAnnotatedValue,
        ok_type: Option<&AnalysedType>,
        err_type: Option<&AnalysedType>,
    ) {
        let err_type = golem_wasm_ast::analysis::protobuf::Type::from(
            err_type.unwrap_or(&AnalysedType::try_from(&inner_element).unwrap()),
        );

        self.push_val(TypeAnnotatedValue::Result(Box::new(
            golem_wasm_rpc::protobuf::TypedResult {
                result_value: Some(
                    golem_wasm_rpc::protobuf::typed_result::ResultValue::ErrorValue(Box::new(
                        golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                            type_annotated_value: Some(inner_element),
                        },
                    )),
                ),
                ok: ok_type.map(golem_wasm_ast::analysis::protobuf::Type::from),
                error: Some(err_type),
            },
        )));
    }

    pub fn push_list(
        &mut self,
        values: Vec<TypeAnnotatedValue>,
        list_elem_type: &AnalysedType, // Expecting a list type and not inner
    ) {
        self.push_val(TypeAnnotatedValue::List(TypedList {
            values: values
                .into_iter()
                .map(|x| golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(x),
                })
                .collect(),
            typ: Some(golem_wasm_ast::analysis::protobuf::Type::from(
                list_elem_type,
            )),
        }));
    }

    pub fn push_tuple(&mut self, values: Vec<TypeAnnotatedValue>, types: &[AnalysedType]) {
        self.push_val(TypeAnnotatedValue::Tuple(TypedTuple {
            value: values
                .into_iter()
                .map(|x| golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(x),
                })
                .collect(),
            typ: types
                .iter()
                .map(golem_wasm_ast::analysis::protobuf::Type::from)
                .collect(),
        }));
    }
}
