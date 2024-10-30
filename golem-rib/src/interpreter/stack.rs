// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
use crate::{GetLiteralValue, LiteralValue};
use golem_wasm_ast::analysis::protobuf::NameTypePair;
use golem_wasm_ast::analysis::{AnalysedType, NameOptionTypePair};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::{
    TypedEnum, TypedList, TypedOption, TypedRecord, TypedTuple, TypedVariant,
};

#[derive(Debug)]
pub struct InterpreterStack {
    pub stack: Vec<RibInterpreterStackValue>,
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

    pub fn pop(&mut self) -> Option<RibInterpreterStackValue> {
        self.stack.pop()
    }

    pub fn try_pop(&mut self) -> Result<RibInterpreterStackValue, String> {
        self.pop()
            .ok_or("Internal Error: Failed to pop value from the interpreter stack".to_string())
    }

    pub fn pop_sink(&mut self) -> Option<Vec<TypeAnnotatedValue>> {
        match self.pop() {
            Some(RibInterpreterStackValue::Sink(vec, _)) => Some(vec.clone()),
            _ => None,
        }
    }

    pub fn pop_n(&mut self, n: usize) -> Option<Vec<RibInterpreterStackValue>> {
        let mut results = Vec::new();
        for _ in 0..n {
            results.push(self.stack.pop()?);
        }
        Some(results)
    }

    pub fn try_pop_n(&mut self, n: usize) -> Result<Vec<RibInterpreterStackValue>, String> {
        self.pop_n(n).ok_or(format!(
            "Internal Error: Failed to pop {} values from the interpreter stack",
            n
        ))
    }

    pub fn try_pop_n_val(&mut self, n: usize) -> Result<Vec<TypeAnnotatedValue>, String> {
        let stack_values = self.try_pop_n(n)?;

        stack_values
            .iter()
            .map(|interpreter_result| {
                interpreter_result
                    .get_val()
                    .ok_or(format!("Internal Error: Failed to convert last {} in the stack to type_annotated_value", n))
            })
            .collect::<Result<Vec<TypeAnnotatedValue>, String>>()
    }

    pub fn try_pop_n_literals(&mut self, n: usize) -> Result<Vec<LiteralValue>, String> {
        let values = self.try_pop_n_val(n)?;
        values
            .iter()
            .map(|type_value| {
                type_value.get_literal().ok_or(format!(
                    "Internal Error: Failed to convert last {} in the stack to literals",
                    n
                ))
            })
            .collect::<Result<Vec<_>, String>>()
    }

    pub fn pop_str(&mut self) -> Option<String> {
        self.pop_val().and_then(|v| match v {
            TypeAnnotatedValue::Str(s) => Some(s),
            _ => None,
        })
    }

    pub fn pop_val(&mut self) -> Option<TypeAnnotatedValue> {
        self.stack.pop().and_then(|v| v.get_val())
    }

    pub fn try_pop_val(&mut self) -> Result<TypeAnnotatedValue, String> {
        self.try_pop().and_then(|x| {
            x.get_val().ok_or(
                "Internal Error: Failed to pop type_annotated_value from the interpreter stack"
                    .to_string(),
            )
        })
    }

    pub fn try_pop_record(&mut self) -> Result<TypedRecord, String> {
        let value = self.try_pop_val()?;

        match value {
            TypeAnnotatedValue::Record(record) => Ok(record),

            _ => Err("Internal Error: Failed to pop a record from the interpreter".to_string()),
        }
    }

    pub fn try_pop_bool(&mut self) -> Result<bool, String> {
        self.try_pop_val().and_then(|val| {
            val.get_literal().and_then(|x| x.get_bool()).ok_or(
                "Internal Error: Failed to pop boolean from the interpreter stack".to_string(),
            )
        })
    }

    pub fn push(&mut self, interpreter_result: RibInterpreterStackValue) {
        self.stack.push(interpreter_result);
    }

    pub fn create_sink(&mut self, analysed_type: &AnalysedType) {
        self.stack.push(RibInterpreterStackValue::Sink(
            vec![],
            analysed_type.clone(),
        ))
    }

    pub fn push_val(&mut self, element: TypeAnnotatedValue) {
        self.stack.push(RibInterpreterStackValue::val(element));
    }

    pub fn push_to_sink(&mut self, type_annotated_value: TypeAnnotatedValue) -> Result<(), String> {
        let sink = self.pop();
        // sink always followed by an iterator
        let possible_iterator = self
            .pop()
            .ok_or("Failed to get the iterator before pushing to the sink")?;

        if !possible_iterator.is_iterator() {
            return Err("Expecting an the iterator before pushing to the sink".to_string());
        }

        match sink {
            Some(RibInterpreterStackValue::Sink(mut list, analysed_type)) => {
                list.push(type_annotated_value);
                self.push(possible_iterator);
                self.push(RibInterpreterStackValue::Sink(list, analysed_type));
                Ok(())
            }

            a => Err(format!(
                "Internal error: Failed to push values to sink {:?}",
                a
            )),
        }
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

    pub fn push_enum(&mut self, enum_name: String, typ: Vec<String>) {
        self.push_val(TypeAnnotatedValue::Enum(TypedEnum {
            typ,
            value: enum_name,
        }))
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
