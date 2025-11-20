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

use crate::golem_agentic::golem::agent::common::{BinaryReference, ElementValue};
use crate::golem_agentic::golem::agent::common::{ElementSchema, TextReference};
use crate::value_and_type::FromValueAndType;
use crate::value_and_type::IntoValue;
use crate::wasm_rpc::WitValue;
use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;
use golem_wasm::Value;

pub trait Schema {
    fn get_type() -> SchemaType;
    fn to_element_value(self) -> Result<ValueType, String>;
    fn from_element_value(value: ValueType, schema: SchemaType) -> Result<Self, String>
    where
        Self: Sized;

    fn from_wit_value(wit_value: WitValue, schema: SchemaType) -> Result<Self, String>
    where
        Self: Sized;

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized;
}

#[derive(Debug)]
pub enum SchemaType {
    Default(ElementSchema),
    Multimodal(Vec<(String, ElementSchema)>),
}

impl SchemaType {
    pub fn get_element_schema(self) -> Option<ElementSchema> {
        match self {
            SchemaType::Default(element_schema) => Some(element_schema),
            SchemaType::Multimodal(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum ValueType {
    Default(ElementValue),
    Multimodal(Vec<(String, ElementValue)>),
}

impl ValueType {
    pub fn get_element_value(self) -> Option<ElementValue> {
        match self {
            ValueType::Default(element_value) => Some(element_value),
            ValueType::Multimodal(_) => None,
        }
    }
    
    pub fn get_multimodal_value(self) -> Option<Vec<(String, ElementValue)>> {
        match self {
            ValueType::Default(_) => None,
            ValueType::Multimodal(multimodal_values) => Some(multimodal_values),
        }
    }
}

// Handles the component model types via the IntoValue and FromValueAndType traits
// This doesn't cover UnstructuredText, UnstructuredBinary
impl<T: IntoValue + FromValueAndType> Schema for T {
    fn get_type() -> SchemaType {
        SchemaType::Default(ElementSchema::ComponentModel(T::get_type()))
    }

    fn to_element_value(self) -> Result<ValueType, String> {
        let wit_value = self.into_value();
        Ok(ValueType::Default(ElementValue::ComponentModel(wit_value)))
    }

    fn from_element_value(value: ValueType, schema: SchemaType) -> Result<Self, String> {
        match value {
            ValueType::Default(ElementValue::ComponentModel(wit_value)) => {
                T::from_wit_value(wit_value, schema)
            }
            _ => Err(format!("Expected ComponentModel value, got: {:?}", value)),
        }
    }

    fn from_wit_value(wit_value: WitValue, schema: SchemaType) -> Result<Self, String> {
        let value_and_type = ValueAndType {
            value: wit_value,
            typ: match schema {
                SchemaType::Default(ElementSchema::ComponentModel(wit_type)) => wit_type,
                _ => return Err(format!("Expected ComponentModel schema, got: {:?}", schema)),
            },
        };

        T::from_value_and_type(value_and_type)
    }

    fn to_wit_value(self) -> Result<WitValue, String> {
        let value_out = self.to_element_value()?;

        let element_value_result = match value_out {
            ValueType::Default(element_value) => Ok(element_value),
            ValueType::Multimodal(_) => {
                Err("Expected element value but found multimodal".to_string())
            }
        };

        let element_value = element_value_result?;

        match element_value {
            ElementValue::ComponentModel(wit_value) => Ok(wit_value),
            ElementValue::UnstructuredBinary(binary_reference) => match binary_reference {
                BinaryReference::Url(url) => {
                    let value = Value::Variant {
                        case_idx: 0,
                        case_value: Some(Box::new(Value::String(url))),
                    };

                    Ok(WitValue::from(value))
                }
                BinaryReference::Inline(binary_source) => {
                    let restriction_record =
                        Value::Record(vec![Value::String(binary_source.binary_type.mime_type)]);

                    let value = Value::Variant {
                        case_idx: 1,
                        case_value: Some(Box::new(Value::Record(vec![
                            Value::List(binary_source.data.into_iter().map(Value::U8).collect()),
                            restriction_record,
                        ]))),
                    };

                    Ok(WitValue::from(value))
                }
            },
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

pub trait MultimodalSchema {
    fn get_multimodal_schema() -> Vec<(String, ElementSchema)>;

    fn get_name(&self) -> String;

    fn to_element_value(self) -> Result<(String, ElementValue), String>
    where
        Self: Sized;

    fn from_element_value(elem: (String, ElementValue)) -> Result<Self, String>
    where
        Self: Sized;

    fn from_wit_value(wit_value: (String, WitValue)) -> Result<Self, String>
    where
        Self: Sized;

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized;
}
