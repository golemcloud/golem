// Copyright 2024-2025 Golem Cloud
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
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{Value, ValueAndType};

#[derive(Debug)]
pub enum RibResult {
    Unit,
    Val(ValueAndType),
}

impl RibResult {
    pub fn from_rib_interpreter_stack_value(
        stack_value: &RibInterpreterStackValue,
    ) -> Option<RibResult> {
        match stack_value {
            RibInterpreterStackValue::Unit => Some(RibResult::Unit),
            RibInterpreterStackValue::Val(value_and_type) => {
                Some(RibResult::Val(value_and_type.clone()))
            }
            RibInterpreterStackValue::Iterator(_) => None,
            RibInterpreterStackValue::Sink(_, _) => None,
        }
    }

    pub fn get_bool(&self) -> Option<bool> {
        match self {
            RibResult::Val(ValueAndType {
                value: Value::Bool(bool),
                ..
            }) => Some(*bool),
            RibResult::Val(_) => None,
            RibResult::Unit => None,
        }
    }
    pub fn get_val(&self) -> Option<ValueAndType> {
        match self {
            RibResult::Val(val) => Some(val.clone()),
            RibResult::Unit => None,
        }
    }

    pub fn get_literal(&self) -> Option<LiteralValue> {
        self.get_val().and_then(|x| x.get_literal())
    }

    pub fn get_record(&self) -> Option<Vec<(String, ValueAndType)>> {
        self.get_val().and_then(|x| match x {
            ValueAndType {
                value: Value::Record(field_values),
                typ: AnalysedType::Record(typ),
            } => Some(
                field_values
                    .into_iter()
                    .zip(typ.fields)
                    .map(|(value, typ)| (typ.name, ValueAndType::new(value, typ.typ)))
                    .collect(),
            ),
            _ => None,
        })
    }
}
