// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
use crate::interpreter::rib_runtime_error::{
    empty_stack, insufficient_stack_items, type_mismatch_with_value,
};
use crate::{internal_corrupted_state, GetLiteralValue, RibInterpreterResult, TypeHint};
use golem_wasm_ast::analysis::analysed_type::{list, option, record, str, tuple, variant};
use golem_wasm_ast::analysis::{
    AnalysedType, NameOptionTypePair, NameTypePair, TypeEnum, TypeRecord, TypeResult,
};
use golem_wasm_rpc::{Value, ValueAndType};

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
    pub fn create_record(&mut self, fields: Vec<NameTypePair>) {
        self.push_val(ValueAndType::new(
            Value::Record(
                vec![Value::Tuple(vec![]); fields.len()], // pre-initializing with () values, to be replaced later by UpdateRecord instructions
            ),
            record(fields),
        ));
    }

    pub fn pop(&mut self) -> Option<RibInterpreterStackValue> {
        self.stack.pop()
    }

    pub fn try_pop(&mut self) -> RibInterpreterResult<RibInterpreterStackValue> {
        self.pop().ok_or(empty_stack())
    }

    pub fn pop_sink(&mut self) -> Option<(Vec<ValueAndType>, AnalysedType)> {
        while let Some(value) = self.pop() {
            match value {
                RibInterpreterStackValue::Sink(vec, analysed_type) => {
                    // We found a sink, return it
                    return Some((vec.clone(), analysed_type.clone()));
                }
                _ => continue, // Keep popping until we find a sink
            }
        }

        None
    }

    pub fn pop_n(&mut self, n: usize) -> Option<Vec<RibInterpreterStackValue>> {
        let mut results = Vec::new();
        for _ in 0..n {
            results.push(self.stack.pop()?);
        }
        Some(results)
    }

    pub fn try_pop_n(&mut self, n: usize) -> RibInterpreterResult<Vec<RibInterpreterStackValue>> {
        self.pop_n(n).ok_or(insufficient_stack_items(n))
    }

    pub fn try_pop_n_val(&mut self, n: usize) -> RibInterpreterResult<Vec<ValueAndType>> {
        let stack_values = self.try_pop_n(n)?;

        stack_values
            .iter()
            .map(|interpreter_result| {
                interpreter_result
                    .get_val()
                    .ok_or(internal_corrupted_state!(
                        "failed to convert last {} in the stack to ValueAndType",
                        n
                    ))
            })
            .collect::<RibInterpreterResult<Vec<ValueAndType>>>()
    }

    pub fn pop_str(&mut self) -> Option<String> {
        self.pop_val().and_then(|v| match v {
            ValueAndType {
                value: Value::String(s),
                ..
            } => Some(s),
            _ => None,
        })
    }

    pub fn pop_val(&mut self) -> Option<ValueAndType> {
        self.stack.pop().and_then(|v| v.get_val())
    }

    pub fn try_pop_val(&mut self) -> RibInterpreterResult<ValueAndType> {
        self.try_pop().and_then(|x| {
            x.get_val().ok_or(internal_corrupted_state!(
                "failed to pop ValueAndType from the interpreter stack"
            ))
        })
    }

    pub fn try_pop_record(&mut self) -> RibInterpreterResult<(Vec<Value>, TypeRecord)> {
        let value = self.try_pop_val()?;

        match value {
            ValueAndType {
                value: Value::Record(field_values),
                typ: AnalysedType::Record(typ),
            } => Ok((field_values, typ)),
            _ => Err(type_mismatch_with_value(
                vec![TypeHint::Record(None)],
                value.value.clone(),
            )),
        }
    }

    pub fn try_pop_bool(&mut self) -> RibInterpreterResult<bool> {
        self.try_pop_val().and_then(|val| {
            val.get_literal()
                .and_then(|x| x.get_bool())
                .ok_or(type_mismatch_with_value(
                    vec![TypeHint::Boolean],
                    val.value.clone(),
                ))
        })
    }

    pub fn push(&mut self, interpreter_result: RibInterpreterStackValue) {
        self.stack.push(interpreter_result);
    }

    pub fn create_sink(&mut self, analysed_type: AnalysedType) {
        self.stack
            .push(RibInterpreterStackValue::Sink(vec![], analysed_type))
    }

    pub fn push_val(&mut self, element: ValueAndType) {
        self.stack.push(RibInterpreterStackValue::val(element));
    }

    pub fn push_to_sink(&mut self, value_and_type: ValueAndType) -> RibInterpreterResult<()> {
        let (mut list, analysed_type) = self.pop_sink().ok_or(internal_corrupted_state!(
            "failed to pop a sink from the interpreter stack"
        ))?;

        list.push(value_and_type);
        self.push(RibInterpreterStackValue::Sink(list, analysed_type));
        Ok(())
    }

    pub fn push_variant(
        &mut self,
        variant_name: String,
        optional_variant_value: Option<Value>,
        cases: Vec<NameOptionTypePair>,
    ) -> RibInterpreterResult<()> {
        let case_idx = cases
            .iter()
            .position(|case| case.name == variant_name)
            .ok_or(internal_corrupted_state!(
                "failed to find the variant {}",
                variant_name
            ))? as u32;

        let case_value = optional_variant_value.map(Box::new);
        self.push_val(ValueAndType::new(
            Value::Variant {
                case_idx,
                case_value,
            },
            variant(cases),
        ));

        Ok(())
    }

    pub fn push_enum(&mut self, enum_name: String, cases: Vec<String>) -> RibInterpreterResult<()> {
        let idx = cases.iter().position(|x| x == &enum_name).ok_or_else(|| {
            internal_corrupted_state!("failed to find the enum {} in the cases", enum_name)
        })? as u32;
        self.push_val(ValueAndType::new(
            Value::Enum(idx),
            AnalysedType::Enum(TypeEnum {
                cases: cases.into_iter().collect(),
                name: None,
                owner: None,
            }),
        ));

        Ok(())
    }

    pub fn push_some(&mut self, inner_element: Value, inner_type: &AnalysedType) {
        self.push_val(ValueAndType {
            value: Value::Option(Some(Box::new(inner_element))),
            typ: option(inner_type.clone()),
        });
    }

    // We allow untyped none to be in stack,
    // Need to verify how strict we should be
    // Example: ${match ok(1) { ok(value) => none }} should be allowed
    pub fn push_none(&mut self, analysed_type: Option<AnalysedType>) {
        self.push_val(ValueAndType {
            value: Value::Option(None),
            typ: option(analysed_type.unwrap_or(str())), // TODO: this used to be a "missing value in protobuf"
        });
    }

    pub fn push_ok(
        &mut self,
        inner_element: Value,
        ok_type: Option<&AnalysedType>,
        err_type: Option<&AnalysedType>,
    ) {
        self.push_val(ValueAndType {
            value: Value::Result(Ok(Some(Box::new(inner_element)))),
            typ: AnalysedType::Result(TypeResult {
                ok: ok_type.map(|x| Box::new(x.clone())),
                err: err_type.map(|x| Box::new(x.clone())),
                name: None,
                owner: None,
            }),
        });
    }

    pub fn push_err(
        &mut self,
        inner_element: Value,
        ok_type: Option<&AnalysedType>,
        err_type: Option<&AnalysedType>,
    ) {
        self.push_val(ValueAndType {
            value: Value::Result(Err(Some(Box::new(inner_element)))),
            typ: AnalysedType::Result(TypeResult {
                ok: ok_type.map(|x| Box::new(x.clone())),
                err: err_type.map(|x| Box::new(x.clone())),
                name: None,
                owner: None,
            }),
        });
    }

    pub fn push_list(&mut self, values: Vec<Value>, list_elem_type: &AnalysedType) {
        self.push_val(ValueAndType {
            value: Value::List(values),
            typ: list(list_elem_type.clone()),
        });
    }

    pub fn push_tuple(&mut self, values: Vec<ValueAndType>) {
        self.push_val(ValueAndType {
            value: Value::Tuple(values.iter().map(|x| x.value.clone()).collect()),
            typ: tuple(values.into_iter().map(|x| x.typ).collect()),
        });
    }
}
