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

use crate::golem_agentic::golem::agent::common::ElementValue;
use crate::golem_agentic::golem::agent::common::{ElementSchema, TextReference};
use crate::value_and_type::FromValueAndType;
use crate::value_and_type::IntoValue;
use crate::wasm_rpc::WitValue;
use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;
use golem_wasm::Value;

pub trait Schema {
    fn get_type() -> ElementSchema;
    fn to_element_value(self) -> Result<ElementValue, String>;
    fn from_element_value(value: ElementValue, schema: ElementSchema) -> Result<Self, String>
    where
        Self: Sized;

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized,
    {
        let element_value = self.to_element_value()?;

        match element_value {
            ElementValue::ComponentModel(wit_value) => Ok(wit_value),
            ElementValue::UnstructuredBinary(_) => {
                todo!("UnstructuredBinary to WitValue conversion not implemented")
            }
            ElementValue::UnstructuredText(text_reference) => match text_reference {
                TextReference::Url(url) => {
                    let value = Value::Variant {
                        case_idx: 0,
                        case_value: Some(Box::new(Value::String(url))),
                    };

                    Ok(WitValue::from(value))
                }

                TextReference::Inline(text_source) => {
                    let restriction_record = text_source.text_type.map(|text_type| {
                        Box::new(Value::Record(vec![Value::String(text_type.language_code)]))
                    });

                    let value = Value::Variant {
                        case_idx: 1,
                        case_value: Some(Box::new(Value::Record(vec![
                            Value::String(text_source.data),
                            Value::Option(restriction_record),
                        ]))),
                    };

                    Ok(WitValue::from(value))
                }
            },
        }
    }
}

impl<T: IntoValue + FromValueAndType> Schema for T {
    fn get_type() -> ElementSchema {
        ElementSchema::ComponentModel(T::get_type())
    }

    fn to_element_value(self) -> Result<ElementValue, String> {
        let wit_value = self.into_value();
        Ok(ElementValue::ComponentModel(wit_value))
    }

    fn from_element_value(value: ElementValue, schema: ElementSchema) -> Result<Self, String> {
        match value {
            ElementValue::ComponentModel(wit_value) => {
                let value_and_type = ValueAndType {
                    value: wit_value,
                    typ: match schema {
                        ElementSchema::ComponentModel(wit_type) => wit_type,
                        _ => {
                            return Err(format!(
                                "Expected ComponentModel schema, got: {:?}",
                                schema
                            ))
                        }
                    },
                };

                T::from_value_and_type(value_and_type)
            }
            _ => Err(format!("Expected ComponentModel value, got: {:?}", value)),
        }
    }
}

pub trait MultimodalSchema {
    fn get_multimodal_schema() -> Vec<(String, ElementSchema)>;

    fn get_name(&self) -> String;

    fn to_element_value(self) -> Result<(String, ElementValue), String>
    where
        Self: Sized;

    fn from_element_value(elem: (String, ElementValue)) -> Result<Self, String>
    where
        Self: Sized;

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized;
}
